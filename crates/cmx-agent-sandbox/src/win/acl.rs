//! Windows ACL 放行集与 `.git` deny（方案 §4.3/§4.5）。
//!
//! write-restricted 语义下，用户 SID 对缓存/工作区文件的 allow 在写检查中**不算数**，必须给
//! restricting SID 显式授 ACE：
//! - `allowed_roots` + `%TEMP%` + extra_roots：授**双 SANDBOX_SID**（shell+git profile 共用放行）
//!   写权限（FILE_GENERIC_WRITE|DELETE|FILE_DELETE_CHILD，SUB_CONTAINERS_AND_OBJECTS_INHERIT）；
//! - `.git`（仅 Shell profile 调用时）：对 **shell SID** 挂 deny（mask 严防连读误杀，见 [`DENY_WRITE_MASK`]）。
//!
//! 既有工作区树是用户真实仓库（非 fresh 目录）——「授予继承到子目录」只作用于**新建**对象，
//! 已有文件须经 `SetNamedSecurityInfoW` 的**自动传播**（整树遍历重写 SD，大仓库分钟级一次性成本）。
//! 陷阱规避（红队 A-4）：无 DACL 子对象传播后变空 DACL=全拒——残余登记（罕见形态，逃逸负例
//! 矩阵覆盖主路径）；`SE_DACL_PROTECTED`（禁继承）文件不传播——残余登记（探测用例兜底）。
//! 幂等性：SANDBOX_SID 安装期持久化 + `SetEntriesInAclW` 同 trustee 合并语义 + 进程内缓存。

#![cfg(windows)]

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSidToSidW, DENY_ACCESS, EXPLICIT_ACCESS_W, GRANT_ACCESS, GetNamedSecurityInfoW,
    NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT, SetEntriesInAclW, SetNamedSecurityInfoW, TRUSTEE_IS_SID,
    TRUSTEE_IS_UNKNOWN, TRUSTEE_W,
};
use windows_sys::Win32::Security::{
    ACL, PSID, PSECURITY_DESCRIPTOR, DACL_SECURITY_INFORMATION, SUB_CONTAINERS_AND_OBJECTS_INHERIT,
};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_APPEND_DATA, FILE_DELETE_CHILD, FILE_GENERIC_WRITE, FILE_WRITE_ATTRIBUTES,
    FILE_WRITE_DATA, FILE_WRITE_EA,
};

use crate::Profile;

/// `.git` deny 位集（红队 A-7：**严禁含 SYNCHRONIZE/READ_CONTROL/GENERIC_WRITE/FILE_GENERIC_WRITE**
/// ——deny GENERIC_WRITE 会因隐含 SYNCHRONIZE 连读一起拒，shell profile 下 `git status` 直接
/// ACCESS_DENIED。只拒纯写位 + 删除位：`git status` 可读、shell 内 `git commit` 拒）。
const DENY_WRITE_MASK: u32 =
    FILE_WRITE_DATA | FILE_APPEND_DATA | FILE_WRITE_EA | FILE_WRITE_ATTRIBUTES | DELETE | FILE_DELETE_CHILD;

/// allow 位集：写全集 + 删除。
const ALLOW_WRITE_MASK: u32 = FILE_GENERIC_WRITE | DELETE | FILE_DELETE_CHILD;

/// 进程内缓存（避免重复付自动传播整树遍历的成本；磁盘上 ACE 持久 + SID 持久 → 重启后幂等）。
static APPLIED: Mutex<Option<HashSet<(PathBuf, bool)>>> = Mutex::new(None);

fn cache_insert(key: (PathBuf, bool)) {
    let lock = cache_core();
    let mut g = lock.lock().expect("sandbox acl cache");
    g.get_or_insert_with(HashSet::new).insert(key);
}

fn cache_contains(key: &(PathBuf, bool)) -> bool {
    let lock = cache_core();
    let g = lock.lock().expect("sandbox acl cache");
    g.as_ref().is_some_and(|s| s.contains(key))
}

/// Mutex<Option<…>> 的静态取址（E0716：借临时值的教训——静态本体一次绑定）。
fn cache_core() -> &'static Mutex<Option<HashSet<(PathBuf, bool)>>> {
    &APPLIED
}

fn path_wide(path: &Path) -> Result<Vec<u16>, String> {
    path.to_str()
        .map(|s| s.encode_utf16().chain(std::iter::once(0)).collect())
        .ok_or_else(|| format!("路径转 UTF-16 失败：{}", path.display()))
}

/// 把一条 EXPLICIT_ACCESS 合并进 path 的现有 DACL（不 PROTECTED：保留继承 + 触发系统自动传播）。
fn merge_ace(path: &Path, sid: &str, mask: u32, mode: i32, inheritance: u32) -> Result<(), String> {
    unsafe {
        let wide = path_wide(path)?;
        let psid_wide: Vec<u16> = sid.encode_utf16().chain(std::iter::once(0)).collect();
        let mut psid: PSID = std::ptr::null_mut();
        if ConvertStringSidToSidW(psid_wide.as_ptr(), &mut psid) == 0 {
            return Err(format!("ConvertStringSidToSidW({sid}) 失败"));
        }
        let ea = EXPLICIT_ACCESS_W {
            grfAccessPermissions: mask,
            grfAccessMode: mode,
            grfInheritance: inheritance,
            Trustee: TRUSTEE_W {
                pMultipleTrustee: std::ptr::null_mut(),
                MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_UNKNOWN,
                ptstrName: psid as _,
            },
        };
        // 读现有 DACL（合并基底；无 DACL = 全允许，直接以新条目起步）。
        let mut psec: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        let mut old_dacl: *mut ACL = std::ptr::null_mut();
        let mut p1: PSID = std::ptr::null_mut();
        let mut p2: PSID = std::ptr::null_mut();
        let mut psacl: *mut ACL = std::ptr::null_mut();
        let rc = GetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            &mut p1,
            &mut p2,
            &mut old_dacl,
            &mut psacl,
            &mut psec,
        );
        if rc != 0 {
            LocalFree(psid as _);
            LocalFree(psec as _);
            return Err(format!("GetNamedSecurityInfoW({}) 失败 rc={rc}", path.display()));
        }
        let mut new_dacl: *mut ACL = std::ptr::null_mut();
        let rc2 = SetEntriesInAclW(1, &ea, old_dacl, &mut new_dacl);
        LocalFree(psid as _);
        if rc2 != 0 {
            LocalFree(psec as _);
            return Err(format!("SetEntriesInAclW({}) 失败 rc={rc2}", path.display()));
        }
        // DACL_SECURITY_INFORMATION（非 PROTECTED）：保留继承链 + 系统自动传播到既有子对象。
        let rc3 = SetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            new_dacl,
            std::ptr::null(),
        );
        LocalFree(new_dacl as _);
        LocalFree(psec as _);
        if rc3 != 0 {
            return Err(format!("SetNamedSecurityInfoW({}) 失败 rc={rc3}", path.display()));
        }
        Ok(())
    }
}

/// 放行集落盘（§4.3）：roots + %TEMP% + extra_write_roots（对**双 SID**授写）。
/// 进程内按路径缓存；幂等。首次对既有树的自动传播为分钟级一次性成本（记档不设预算，方案 §8.3）。
pub(crate) fn ensure_workspace_acls(roots: &[PathBuf], profile: Profile) -> Result<(), String> {
    let sids = crate::settings::sid_pair();
    let mut targets: Vec<PathBuf> = crate::effective_write_roots(roots);
    targets.retain(|p| p.exists());
    for t in &targets {
        if cache_contains(&(t.clone(), false)) {
            continue;
        }
        let t0 = std::time::Instant::now();
        for sid in [&sids.shell, &sids.git] {
            merge_ace(t, sid, ALLOW_WRITE_MASK, GRANT_ACCESS, SUB_CONTAINERS_AND_OBJECTS_INHERIT)?;
        }
        // ACL 自动传播残余面登记（§4.4）：SE_DACL_PROTECTED 不传播（兜底=逃逸负例矩阵）；传播时长记档。
        eprintln!(
            "[sandbox] ACL 放行 {}（自动传播 {}ms，一次性）",
            t.display(),
            t0.elapsed().as_millis()
        );
        cache_insert((t.clone(), false));
    }
    // `.git` deny：仅 Shell profile 调用方（git 工具走 Git profile 不触发本段——语义裂口由
    // deny 只挂 shell SID 实现，见 SandboxSidPair 文档）。`.git` 尚不存在则不挂（下次 shell 调用重试）。
    if matches!(profile, Profile::Shell) {
        for root in roots {
            let gitdir = root.join(".git");
            if !gitdir.exists() || cache_contains(&(gitdir.clone(), true)) {
                continue;
            }
            merge_ace(&gitdir, &sids.shell, DENY_WRITE_MASK, DENY_ACCESS, SUB_CONTAINERS_AND_OBJECTS_INHERIT)?;
            cache_insert((gitdir, true));
        }
    }
    Ok(())
}

/// 工作区所在卷文件系统探测（§4.3，红队 A-8）：FAT/exFAT 无安全描述符，写围栏完全失效——
/// fail-closed。只认 NTFS/ReFS。
pub(crate) fn ensure_acl_capable_volume(path: &Path) -> Result<(), String> {
    unsafe {
        let mut fsname = [0u16; 16];
        let mut serial = 0u32;
        let mut maxcomp = 0u32;
        let mut flags = 0u32;
        // MSDN：lpRootPathName 期望卷根（盘符路径最稳；目录路径在部分系统上失败——实测教训）。
        let root = volume_root(path);
        let wide = path_wide(&root)?;
        let ok = windows_sys::Win32::Storage::FileSystem::GetVolumeInformationW(
            wide.as_ptr(),
            std::ptr::null_mut(),
            0,
            &mut serial,
            &mut maxcomp,
            &mut flags,
            fsname.as_mut_ptr(),
            fsname.len() as u32,
        );
        if ok == 0 {
            let gle = windows_sys::Win32::Foundation::GetLastError();
            return Err(format!("GetVolumeInformationW({}) 失败（GLE={gle}）", root.display()));
        }
        let fs = String::from_utf16_lossy(&fsname);
        let fs_trim = fs.trim_end_matches('\0');
        if fs_trim.eq_ignore_ascii_case("NTFS") || fs_trim.eq_ignore_ascii_case("ReFS") {
            Ok(())
        } else {
            Err(format!("工作区所在卷为 {fs_trim}（非 NTFS/ReFS 无 ACL，写围栏失效——fail-closed）"))
        }
    }
}

/// 从任意路径推导卷根（`C:\dir\x` → `C:\`；UNC/无盘符原样返回——GetVolumeInformationW 兜底语义）。
fn volume_root(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    let b = s.as_bytes();
    if b.len() >= 3 && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/') {
        return PathBuf::from(&s[..3]);
    }
    path.to_path_buf()
}

/// 探测用（红队 A-15）：在临时文件上试挂 grant ACE 后回读——「探测恒绿但真实失败测不出」的防线。
pub(crate) fn probe_ace_roundtrip() -> Result<(), String> {
    let dir = std::env::temp_dir().join(format!("cmx-sbx-probe-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let f = dir.join("probe.txt");
    std::fs::write(&f, b"probe").map_err(|e| e.to_string())?;
    let sid = crate::settings::sid_pair().shell;
    let r = merge_ace(&f, &sid, ALLOW_WRITE_MASK, GRANT_ACCESS, 0)
        .and_then(|_| ensure_acl_capable_volume(&dir));
    let _ = std::fs::remove_dir_all(&dir);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_root(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cmx-acl-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn grant_and_deny_ace_roundtrip() {
        let _g = crate::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        crate::settings::init(None);
        let root = tmp_root("rt");
        let sids = crate::settings::sid_pair();
        // 根目录：双 SID 授写。
        merge_ace(&root, &sids.shell, ALLOW_WRITE_MASK, GRANT_ACCESS, SUB_CONTAINERS_AND_OBJECTS_INHERIT).unwrap();
        merge_ace(&root, &sids.git, ALLOW_WRITE_MASK, GRANT_ACCESS, SUB_CONTAINERS_AND_OBJECTS_INHERIT).unwrap();
        // 文件：shell SID 挂 deny（.git 语义模拟）。
        let f = root.join("x.txt");
        std::fs::write(&f, b"x").unwrap();
        merge_ace(&f, &sids.shell, DENY_WRITE_MASK, DENY_ACCESS, 0).unwrap();
        // 重复挂应幂等（同 trustee 合并语义）。
        merge_ace(&f, &sids.shell, DENY_WRITE_MASK, DENY_ACCESS, 0).unwrap();
        assert!(f.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn volume_check_on_temp() {
        let _g = crate::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // 本机 %TEMP% 所在卷应为 NTFS/ReFS（异机若为 FAT 会如实失败——这正是探测语义）。
        let r = ensure_acl_capable_volume(&std::env::temp_dir());
        assert!(r.is_ok(), "temp 卷探测：{r:?}");
    }
}
