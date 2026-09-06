import json,sys,time,threading,urllib.request
BASE=sys.argv[1].rstrip('/'); OUT=sys.argv[2]; res=[]
def post(o,t=90):
    req=urllib.request.Request(BASE+"/api",data=json.dumps(o).encode(),headers={"Content-Type":"application/json"})
    with urllib.request.urlopen(req,timeout=t) as r: return json.loads(r.read().decode())
def rec(n,desc,ok,note,data=None):
    res.append({"name":n,"desc":desc,"pass":ok,"note":note,"data":data})
    print(f"[{'PASS' if ok else 'FAIL'}] {n:22} {note}")

def stream_turn(sid, text, decision):
    """打开 SSE 跑一个回合；见到 approval_requested 就按 decision 审批。返回收到的事件列表。"""
    post({"cmd":"create_session","id":sid})
    got=[]; approved_call=[None]
    def reader():
        req=urllib.request.Request(BASE+"/api/stream",data=json.dumps({"session_id":sid,"text":text}).encode(),headers={"Content-Type":"application/json"})
        try:
            with urllib.request.urlopen(req,timeout=90) as r:
                for raw in r:
                    line=raw.decode(errors="replace").strip()
                    if not line.startswith("data:"): continue
                    try: ev=json.loads(line[5:].strip())
                    except: continue
                    got.append(ev)
                    if ev.get("kind")=="approval_requested" and approved_call[0] is None:
                        approved_call[0]=ev.get("call_id")
                    if ev.get("kind") in ("stream_done","stream_error"): break
        except Exception as e:
            got.append({"kind":"reader_error","message":str(e)})
    th=threading.Thread(target=reader,daemon=True); th.start()
    # 等 approval_requested
    cid=None
    for _ in range(300):
        if approved_call[0]: cid=approved_call[0]; break
        if any(e.get("kind") in ("stream_done","stream_error") for e in got): break
        time.sleep(0.1)
    approve_resp=None
    if cid:
        approve_resp=post({"cmd":"approve","call_id":cid,"approved":decision,"session_id":sid})
    th.join(timeout=90)
    return got, cid, approve_resp

# X4-A 批准路径
evs,cid,ap = stream_turn("x4-approve","请在当前工作区用 bash 执行命令：echo cmx-x4-ok，然后把命令输出原样告诉我。", True)
kinds=[e.get("kind") for e in evs]
req_seen = "approval_requested" in kinds
bash_tool = any(e.get("kind")=="approval_requested" and e.get("tool")=="bash" for e in evs)
tr=[e for e in evs if e.get("kind")=="tool_result"]
out_ok = any("cmx-x4-ok" in json.dumps(e.get("output"),ensure_ascii=False) for e in tr)
done = "stream_done" in kinds
rec("X4A_approve_bash", "审批批准→bash执行→输出回灌", req_seen and cid and bash_tool and out_ok and done,
    f"req={req_seen} tool=bash:{bash_tool} approved_call={bool(cid)} echo_ok={out_ok} done={done}", {"kinds":kinds})

# X4-B 拒绝路径
evs,cid,ap = stream_turn("x4-reject","请在当前工作区用 bash 执行命令：echo should-not-run，并把输出告诉我。", False)
kinds=[e.get("kind") for e in evs]
req_seen="approval_requested" in kinds
resolved_reject = any(e.get("kind")=="approval_resolved" and e.get("approved")==False for e in evs)
tr=[e for e in evs if e.get("kind")=="tool_result"]
# 拒绝后 bash 不应产出 should-not-run
ran = any("should-not-run" in json.dumps(e.get("output"),ensure_ascii=False) for e in tr)
done = "stream_done" in kinds or "stream_error" in kinds
rec("X4B_reject_bash","审批拒绝→工具被拦截未执行", req_seen and cid and resolved_reject and (not ran) and done,
    f"req={req_seen} rejected={resolved_reject} bash_ran={ran}(应False) done={done}", {"kinds":kinds})

summ={"base":BASE,"total":len(res),"passed":sum(1 for x in res if x['pass']),"failed":sum(1 for x in res if not x['pass']),"results":res}
open(OUT,"w").write(json.dumps(summ,ensure_ascii=False,indent=2))
print(f"\nX4 审批用例: {summ['passed']}/{summ['total']} PASS")
