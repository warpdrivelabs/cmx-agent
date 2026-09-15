//! Windows 受限令牌构造（方案 §4.1/§4.2）。
//!
//! 机制（红队 A-2 修正后的准确表述）：
//! `CreateRestrictedToken(DISABLE_MAX_PRIVILEGE | WRITE_RESTRICTED)`，**不 disable 任何 SID**
//! （DisableSids→deny-only 是另一机制，会把用户 SID 变 deny-only 连读都毁掉——严禁误用）。
//! `WRITE_RESTRICTED` 语义：**写访问额外做一次只认 restricting SIDs 的检查**（对象 DACL 须含
//! 命中 restricting SID 的 allow ACE 才可写）；读/执行走正常检查——「系统其余只读」的准确含义。
//!
//! restricting 集合（方案 §4.2，Codex 同款取舍）：{profile 对应的 SANDBOX_SID, Logon SID, Everyone}。
//! 后两者让令牌 Default DACL / 公共授权对象在写检查中可命中（否则子进程自建管道/IPC 对象
//! 「创建成功但再打开写入即 ACCESS_DENIED」——PowerShell 管道直接崩）。代价：Everyone 可写的
//! 公共目录在沙箱内仍可写，明示接受。Default DACL 经 `SetTokenInformation` 授 Logon+Everyone+
//! 双 SANDBOX_SID（Codex permissive default DACL 同款）。

#![cfg(windows)]

use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_ALL, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSidToSidW, EXPLICIT_ACCESS_W, GRANT_ACCESS,
    NO_MULTIPLE_TRUSTEE, TRUSTEE_IS_SID, TRUSTEE_IS_UNKNOWN, TRUSTEE_W,
};
use windows_sys::Win32::Security::{
    CreateRestrictedToken, GetTokenInformation, SetTokenInformation, TokenDefaultDacl, TokenGroups,
    TOKEN_DEFAULT_DACL, ACL, DISABLE_MAX_PRIVILEGE, SID_AND_ATTRIBUTES, TOKEN_ADJUST_DEFAULT,
    TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_GROUPS, TOKEN_QUERY, WRITE_RESTRICTED,
};
use windows_sys::Win32::System::SystemServices::SE_GROUP_LOGON_ID;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use crate::Profile;

/// Everyone（S-1-1-0，world SID）。
const EVERYONE_SID: &str = "S-1-1-0";

pub(crate) struct OwnedToken(pub usize);

impl Drop for OwnedToken {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0 as _) };
    }
}

/// usize ⇄ HANDLE 边界转换（句柄存储用 usize 保持 Send——对齐 tools/job.rs 惯例）。
fn h(v: usize) -> HANDLE {
    v as _
}

/// 进程自身令牌（仅本函数作用域内持有，OwnedToken 兜底关闭）。
fn own_token() -> Result<OwnedToken, String> {
    unsafe {
        let mut tk: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(
            GetCurrentProcess(),
            // ASSIGN_PRIMARY：CreateProcessAsUserW 的硬性要求（TOKEN_QUERY|DUPLICATE|ASSIGN_PRIMARY）；
            // ADJUST_DEFAULT：SetTokenInformation(DefaultDacl) 需要——派生令牌的访问掩码继承源句柄。
            TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_ADJUST_DEFAULT | TOKEN_ASSIGN_PRIMARY,
            &mut tk,
        ) == 0
        {
            return Err(format!("OpenProcessToken 失败（GLE={}）", windows_sys::Win32::Foundation::GetLastError()));
        }
        Ok(OwnedToken(tk as usize))
    }
}

/// 从令牌提取 Logon SID（S-1-5-5-<logon-session>，会话内恒定）。写检查第二道的兼容成员。
fn logon_sid(token: &OwnedToken) -> Result<String, String> {
    unsafe {
        let mut len: u32 = 0;
        GetTokenInformation(h(token.0), TokenGroups, std::ptr::null_mut(), 0, &mut len);
        if len == 0 {
            return Err("GetTokenInformation(TokenGroups) 长度探测失败".into());
        }
        let mut buf = vec![0u8; len as usize];
        if GetTokenInformation(h(token.0), TokenGroups, buf.as_mut_ptr() as _, len, &mut len) == 0 {
            return Err("GetTokenInformation 失败".into());
        }
        let groups = &*(buf.as_ptr() as *const TOKEN_GROUPS);
        for i in 0..groups.GroupCount as usize {
            let g = &*groups.Groups.as_ptr().add(i);
            if g.Attributes as i32 & SE_GROUP_LOGON_ID != 0 {
                let mut s: windows_sys::core::PWSTR = std::ptr::null_mut();
                if ConvertSidToStringSidW(g.Sid, &mut s) == 0 {
                    return Err("ConvertSidToStringSidW 失败".into());
                }
                let out = pwstr_to_string(s);
                LocalFree(s as _);
                return Ok(out);
            }
        }
        Err("令牌组中未找到 Logon SID（SE_GROUP_LOGON_ID）".into())
    }
}

/// 把 SID 字符串物化成 PSID（LocalAlloc 分配，调用方负责 LocalFree）。
fn materialize_sid(s: &str) -> Result<usize, String> {
    let wide: Vec<u16> = s.encode_utf16().chain(std::iter::once(0)).collect();
    let mut psid: usize = 0;
    if unsafe { ConvertStringSidToSidW(wide.as_ptr(), &mut psid as *mut usize as _) } == 0 {
        return Err(format!("ConvertStringSidToSidW({s}) 失败"));
    }
    Ok(psid)
}

/// 派生受限令牌。失败 = fail-closed（调用方直接拒绝执行）。
/// profile 决定挂哪枚 SANDBOX_SID（`shell` → `.git` deny 生效；`git` → 无 deny，见 settings::SandboxSidPair）。
pub(crate) fn create(profile: Profile) -> Result<OwnedToken, String> {
    let sids = crate::settings::sid_pair();
    let profile_sid = match profile {
        Profile::Shell | Profile::Plugin => sids.shell,
        Profile::Git => sids.git,
    };
    let own = own_token()?;
    let logon = logon_sid(&own)?;
    let sid_strings = [profile_sid, logon, EVERYONE_SID.to_string()];

    let mut restrict: Vec<SID_AND_ATTRIBUTES> = Vec::with_capacity(3);
    let mut owned: Vec<usize> = Vec::with_capacity(3);
    let mut err: Option<String> = None;
    for s in &sid_strings {
        match materialize_sid(s) {
            Ok(psid) => {
                owned.push(psid);
                restrict.push(SID_AND_ATTRIBUTES { Sid: psid as _, Attributes: 0 });
            }
            Err(e) => {
                err = Some(e);
                break;
            }
        }
    }

    let mut newtok: HANDLE = std::ptr::null_mut();
    let result = if let Some(e) = err {
        Err(e)
    } else {
        unsafe {
            // DISABLE_MAX_PRIVILEGE：剥特权；WRITE_RESTRICTED：写检查双闸；绝不 disable 任何 SID（§4.1）。
            let flags = DISABLE_MAX_PRIVILEGE | WRITE_RESTRICTED;
            if CreateRestrictedToken(
                h(own.0),
                flags,
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
                restrict.len() as u32,
                restrict.as_ptr(),
                &mut newtok,
            ) == 0
            {
                Err(format!(
                    "CreateRestrictedToken 失败（GLE={}）",
                    windows_sys::Win32::Foundation::GetLastError()
                ))
            } else {
                let tok = OwnedToken(newtok as usize);
                set_default_dacl(&tok).map(|_| tok)
            }
        }
    };
    for p in owned {
        unsafe { LocalFree(p as _) };
    }
    result
}

/// Default DACL 改写（§4.2）：现 DACL + GRANT(GENERIC_ALL) × {双 SANDBOX_SID, Logon, Everyone}。
/// 让子进程自建的管道/named mutex/IPC 对象在写检查中可写（PowerShell 管道必需）。
/// 失败=令牌不可用（fail-closed）。
fn set_default_dacl(token: &OwnedToken) -> Result<(), String> {
    unsafe {
        let mut len: u32 = 0;
        GetTokenInformation(h(token.0), TokenDefaultDacl, std::ptr::null_mut(), 0, &mut len);
        if len == 0 {
            return Err("GetTokenInformation(TokenDefaultDacl) 探测失败".into());
        }
        let mut buf = vec![0u8; len as usize];
        if GetTokenInformation(h(token.0), TokenDefaultDacl, buf.as_mut_ptr() as _, len, &mut len) == 0 {
            return Err("GetTokenInformation(DefaultDacl) 失败".into());
        }
        let old = &*(buf.as_ptr() as *const TOKEN_DEFAULT_DACL);

        let sids = crate::settings::sid_pair();
        let logon = logon_sid(&own_token()?)?;
        let grants = [sids.shell.clone(), sids.git.clone(), logon, EVERYONE_SID.to_string()];
        let mut entries: Vec<EXPLICIT_ACCESS_W> = Vec::with_capacity(grants.len());
        let mut owned: Vec<usize> = Vec::new();
        for g in &grants {
            match materialize_sid(g) {
                Ok(psid) => {
                    owned.push(psid);
                    entries.push(EXPLICIT_ACCESS_W {
                        grfAccessPermissions: GENERIC_ALL,
                        grfAccessMode: GRANT_ACCESS,
                        grfInheritance: 0,
                        Trustee: TRUSTEE_W {
                            pMultipleTrustee: std::ptr::null_mut(),
                            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
                            TrusteeForm: TRUSTEE_IS_SID,
                            TrusteeType: TRUSTEE_IS_UNKNOWN,
                            ptstrName: psid as _,
                        },
                    });
                }
                Err(e) => {
                    for p in owned {
                        LocalFree(p as _);
                    }
                    return Err(e);
                }
            }
        }
        let mut new_dacl: *mut ACL = std::ptr::null_mut();
        let rc = windows_sys::Win32::Security::Authorization::SetEntriesInAclW(
            entries.len() as u32,
            entries.as_ptr(),
            old.DefaultDacl,
            &mut new_dacl,
        );
        for p in owned {
            LocalFree(p as _);
        }
        if rc != 0 {
            return Err(format!("SetEntriesInAclW(DefaultDacl) 失败（rc={rc}）"));
        }
        let info = TOKEN_DEFAULT_DACL { DefaultDacl: new_dacl };
        let r = SetTokenInformation(
            h(token.0),
            TokenDefaultDacl,
            &info as *const _ as *const _,
            std::mem::size_of::<TOKEN_DEFAULT_DACL>() as u32,
        );
        LocalFree(new_dacl as _);
        if r == 0 {
            return Err("SetTokenInformation(DefaultDacl) 失败".into());
        }
        Ok(())
    }
}

/// 能力探测（§3.3，红队 A-15）：真派生一次受限令牌 + 试挂 ACE 后回读，防「探测恒绿」。
pub(crate) fn probe_token_derivation() -> crate::probe::Availability {
    match create(Profile::Shell) {
        Ok(_) => match crate::win::acl::probe_ace_roundtrip() {
            Ok(()) => crate::probe::Availability::Available,
            Err(e) => crate::probe::Availability::Unavailable(format!("试挂 ACE 失败：{e}")),
        },
        Err(e) => crate::probe::Availability::Unavailable(format!("受限令牌派生失败：{e}")),
    }
}

pub(crate) fn pwstr_to_string(p: windows_sys::core::PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut len = 0usize;
    unsafe {
        while *p.add(len) != 0 {
            len += 1;
        }
    }
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_derivation_and_default_dacl() {
        let _g = crate::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // 普通桌面用户（本测试进程即普通令牌）下完整走通：派生 + Default DACL 改写。
        // 这是 S1a 首验收项的单元层（集成层见 win::spawn 测试）。
        crate::settings::init(None);
        let tok = create(Profile::Shell)
            .expect("受限令牌派生（自派生令牌无需 SeAssignPrimaryTokenPrivilege，codex 产线同款）");
        drop(tok);
        let _ = create(Profile::Git).expect("git profile 令牌派生");
    }

    #[test]
    fn logon_sid_shape() {
        let _g = crate::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        crate::settings::init(None);
        let own = own_token().unwrap();
        let s = logon_sid(&own).unwrap();
        assert!(s.starts_with("S-1-5-5-"), "Logon SID 形态：{s}");
    }
}
