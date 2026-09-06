import json, sys, time, urllib.request
BASE = sys.argv[1].rstrip('/'); OUT = sys.argv[2]
results = []
def call(o, timeout=45):
    req = urllib.request.Request(BASE+"/api", data=json.dumps(o).encode(), headers={"Content-Type":"application/json"})
    t0=time.time()
    with urllib.request.urlopen(req, timeout=timeout) as r: return json.loads(r.read().decode()), round((time.time()-t0)*1000)
def health():
    with urllib.request.urlopen(BASE+"/health", timeout=10) as r: return r.read().decode()
def rec(name,desc,req,resp,ms,passed,note=""):
    results.append({"name":name,"desc":desc,"request":req,"response":resp,"ms":ms,"pass":passed,"note":note})
    print(f"[{'PASS' if passed else 'FAIL'}] {name:24} {ms:>6}ms  {note}")
def fresh(sid, text, timeout=45):
    call({"cmd":"create_session","id":sid})
    return call({"cmd":"send","session_id":sid,"text":text}, timeout)
def events_of(sendresp):
    d=sendresp.get("data") or {}
    return d.get("new_events") or []
def find_kind(evs,kind):
    return [e for e in evs if e.get("kind")==kind]

# F1 health
try:
    h=health(); rec("F1_health","GET /health",{"GET":"/health"},h,0,h.strip()=="ok")
except Exception as e: rec("F1_health","GET /health",{},str(e),0,False)

# F2 current_user unauth
r,ms=call({"cmd":"current_user"}); rec("F2_current_user_unauth","未登录取用户",{"cmd":"current_user"},r,ms,r.get("ok")==True and (r.get("data") or {}).get("user") in (None,""))

# F3 create + F4 list
r,ms=call({"cmd":"create_session","id":"ftest-basic"}); rec("F3_create_session","新建会话",{"cmd":"create_session"},r,ms,r.get("ok")==True)
r,ms=call({"cmd":"list_sessions"}); rec("F4_list_sessions","列会话含新建",{"cmd":"list_sessions"},r,ms,"ftest-basic" in json.dumps(r,ensure_ascii=False))

# F5 add (fresh) — 断言 tool_result.output.sum==5
r,ms=fresh("ft-add","算 2 加 3"); evs=events_of(r)
tr=find_kind(evs,"tool_result")
sum_ok = any((e.get("output") or {}).get("sum")==5.0 for e in tr)
inv=find_kind(evs,"tool_invoked"); add_called=any(e.get("call",{}).get("name")=="add" for e in inv)
rec("F5_add_tool","加法回合(fresh):add→sum5",{"text":"算 2 加 3"},r,ms, r.get("ok")==True and add_called and sum_ok, f"add_called={add_called} sum5={sum_ok}")

# F6 get_events 事件类型齐全
r2,ms2=call({"cmd":"get_events","session_id":"ft-add"}); evs2=(r2.get("data") or {}).get("events") or []
kinds=set(e.get("kind") for e in evs2)
need={"turn_started","user_message","model_message","tool_invoked","tool_result","turn_ended"}
rec("F6_get_events","事件类型齐全",{"cmd":"get_events"},r2,ms2, r2.get("ok")==True and need.issubset(kinds), f"kinds={sorted(kinds)}")

# F7 clock (fresh)
r,ms=fresh("ft-clock","现在几点了"); evs=events_of(r); inv=find_kind(evs,"tool_invoked")
clock_called=any(e.get("call",{}).get("name")=="clock" for e in inv)
rec("F7_clock_tool","时钟回合(fresh):clock",{"text":"现在几点了"},r,ms, r.get("ok")==True and clock_called, f"clock_called={clock_called}")

# F8 flow connector (fresh, live :8091)
r,ms=fresh("ft-flow","列出所有流程定义"); evs=events_of(r); inv=find_kind(evs,"tool_invoked"); tr=find_kind(evs,"tool_result")
flow_called=any(e.get("call",{}).get("name")=="flow_list_definitions" for e in inv)
tr_ok = tr[0].get("ok") if tr else None
tr_out = json.dumps(tr[0].get("output"),ensure_ascii=False)[:160] if tr else ""
rec("F8_flow_connector","flow连接器→live:8091",{"text":"列出所有流程定义"},r,ms, r.get("ok")==True and flow_called, f"called={flow_called} tool_ok={tr_ok} out={tr_out}")

# F9 onto connector (fresh, live :8097)
r,ms=fresh("ft-onto","列出对象类型"); evs=events_of(r); inv=find_kind(evs,"tool_invoked"); tr=find_kind(evs,"tool_result")
onto_called=any(e.get("call",{}).get("name")=="onto_list_object_types" for e in inv)
tr_ok=tr[0].get("ok") if tr else None; tr_out=json.dumps(tr[0].get("output"),ensure_ascii=False)[:160] if tr else ""
rec("F9_onto_connector","onto连接器→live:8097",{"text":"列出对象类型"},r,ms, r.get("ok")==True and onto_called, f"called={onto_called} tool_ok={tr_ok} out={tr_out}")

# F10 report connector (fresh, live :8092)
r,ms=fresh("ft-report","列出报表"); evs=events_of(r); inv=find_kind(evs,"tool_invoked"); tr=find_kind(evs,"tool_result")
rep_called=any(e.get("call",{}).get("name")=="report_list_reports" for e in inv)
tr_ok=tr[0].get("ok") if tr else None; tr_out=json.dumps(tr[0].get("output"),ensure_ascii=False)[:160] if tr else ""
rec("F10_report_connector","report连接器→live:8092",{"text":"列出报表"},r,ms, r.get("ok")==True and rep_called, f"called={rep_called} tool_ok={tr_ok} out={tr_out}")

# F11 text fallback (fresh, 无关键词)
r,ms=fresh("ft-fb","你好呀"); evs=events_of(r); mm=find_kind(evs,"model_message")
fb_ok=any("演示模型" in (e.get("text") or "") for e in mm) and not find_kind(evs,"tool_invoked")
rec("F11_text_fallback","无关键词→纯文本兜底(fresh)",{"text":"你好呀"},r,ms, r.get("ok")==True and fb_ok, f"fallback_text={fb_ok}")

# F12 pagination
r,ms=call({"cmd":"get_events","session_id":"ft-add","limit":2}); d=r.get("data") or {}; evs=d.get("events")
rec("F12_pagination","分页尾加载 limit=2",{"cmd":"get_events","limit":2},r,ms, r.get("ok")==True and isinstance(evs,list) and len(evs)<=2, f"total={d.get('total')} got={len(evs) if isinstance(evs,list) else '?'}")

# F13 list_connectors
r,ms=call({"cmd":"list_connectors"}); blob=json.dumps(r,ensure_ascii=False)
rec("F13_list_connectors","连接器面板+live健康",{"cmd":"list_connectors"},r,ms, r.get("ok")==True and all(k in blob for k in ["flow","onto","report"]))

# F14 login live portal
r,ms=call({"cmd":"login","username":"admin","password":"Admin@12345"}); rec("F14_login_live","门户真登录 admin",{"cmd":"login"},r,ms, r.get("ok")==True)

# F15 current_user after login
r,ms=call({"cmd":"current_user"}); u=(r.get("data") or {}).get("user"); rec("F15_current_user_auth","登录后取用户",{"cmd":"current_user"},r,ms, r.get("ok")==True and bool(u), f"user={u if isinstance(u,str) else (u or {}) }")

# F16 logout
r,ms=call({"cmd":"logout"}); rec("F16_logout","登出",{"cmd":"logout"},r,ms, r.get("ok")==True)
r2,_=call({"cmd":"current_user"}); rec("F16b_logout_verify","登出后用户空",{"cmd":"current_user"},r2,0,(r2.get("data") or {}).get("user") in (None,""))

# F17 delete + verify
r,ms=call({"cmd":"delete_session","session_id":"ft-add"}); r2,_=call({"cmd":"list_sessions"})
rec("F17_delete_session","删会话并校验消失",{"cmd":"delete_session"},{"del":r,"list":r2},ms, r.get("ok")==True and "ft-add" not in json.dumps(r2,ensure_ascii=False))

# F18 bad request（未知 cmd → 优雅错误信封）
try:
    r,ms=call({"cmd":"no_such_command"}); rec("F18_bad_cmd","未知命令→错误信封",{"cmd":"no_such_command"},r,ms, r.get("ok")==False and (r.get("error") or {}).get("code") is not None)
except Exception as e:
    rec("F18_bad_cmd","未知命令→错误信封",{"cmd":"no_such_command"},str(e),0,False,f"HTTP层异常 {e}")

summ={"base":BASE,"total":len(results),"passed":sum(1 for x in results if x["pass"]),"failed":sum(1 for x in results if not x["pass"]),"results":results}
open(OUT,"w").write(json.dumps(summ,ensure_ascii=False,indent=2))
print(f"\n功能用例: {summ['passed']}/{summ['total']} PASS, {summ['failed']} FAIL")
