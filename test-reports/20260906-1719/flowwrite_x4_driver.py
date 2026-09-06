import json,sys,time,threading,urllib.request
BASE=sys.argv[1].rstrip('/'); OUT=sys.argv[2]
def post(o,t=120):
    r=urllib.request.Request(BASE+"/api",data=json.dumps(o).encode(),headers={"Content-Type":"application/json"})
    with urllib.request.urlopen(r,timeout=t) as x: return json.loads(x.read().decode())

# 登录（token 入共享槽 → 连接器带 Bearer）
print("login:", post({"cmd":"login","username":"admin","password":"Admin@12345"}).get("ok"))
sid="cap-flowwrite"; post({"cmd":"create_session","id":sid})
got=[]; cid=[None]
def reader():
    text="请用 flow_start_instance 工具起一个流程实例，definitionKey 用 s5_voucher，variables 里 amount=6666。"
    req=urllib.request.Request(BASE+"/api/stream",data=json.dumps({"session_id":sid,"text":text}).encode(),headers={"Content-Type":"application/json"})
    try:
        with urllib.request.urlopen(req,timeout=120) as r:
            for raw in r:
                line=raw.decode(errors="replace").strip()
                if not line.startswith("data:"): continue
                try: ev=json.loads(line[5:].strip())
                except: continue
                got.append(ev)
                if ev.get("kind")=="approval_requested" and cid[0] is None: cid[0]=ev.get("call_id")
                if ev.get("kind") in ("stream_done","stream_error"): break
    except Exception as e: got.append({"kind":"reader_error","message":str(e)})
th=threading.Thread(target=reader,daemon=True); th.start()
# 等审批请求 → 批准
c=None
for _ in range(400):
    if cid[0]: c=cid[0]; break
    if any(e.get("kind") in ("stream_done","stream_error") for e in got): break
    time.sleep(0.1)
ap=None
if c: ap=post({"cmd":"approve","call_id":c,"approved":True,"session_id":sid})
th.join(timeout=120)

kinds=[e.get("kind") for e in got]
areq=[e for e in got if e.get("kind")=="approval_requested"]
tr=[e for e in got if e.get("kind")=="tool_result"]
started = any((e.get("output") or {}).get("started")==True for e in tr)
iid=None
for e in tr:
    o=e.get("output") or {}
    if o.get("started"): iid=o.get("instanceId") or (o.get("data") or {}).get("id")
tool_names=[e.get("tool") for e in areq]
res={"login_first":True,"approval_tool":tool_names,"approved_call":bool(c),
     "flow_start_called":any(t=="flow_start_instance" for t in tool_names),
     "started":started,"instanceId":iid,"kinds":kinds,
     "final_text":next((e.get("text") for e in reversed(got) if e.get("kind")=="model_message"),"")}
open(OUT,"w").write(json.dumps(res,ensure_ascii=False,indent=2))
ok = res["flow_start_called"] and res["approved_call"] and res["started"] and iid
print(f"\n[{'PASS' if ok else 'FAIL'}] 端到端护城河：真模型→flow_start_instance→X4批准→活体起实例")
print(f"  审批工具={tool_names} started={started} instanceId={iid}")
print(f"  final: {res['final_text'][:90]}")
