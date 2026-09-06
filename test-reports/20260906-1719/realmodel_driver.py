import json,sys,time,urllib.request
BASE=sys.argv[1].rstrip('/'); OUT=sys.argv[2]; res=[]
def call(o,t=120):
    req=urllib.request.Request(BASE+"/api",data=json.dumps(o).encode(),headers={"Content-Type":"application/json"})
    t0=time.time()
    with urllib.request.urlopen(req,timeout=t) as r: return json.loads(r.read().decode()),round((time.time()-t0)*1000)
def rec(n,desc,req,resp,ms,ok,note=""):
    res.append({"name":n,"desc":desc,"request":req,"response":resp,"ms":ms,"pass":ok,"note":note})
    print(f"[{'PASS' if ok else 'FAIL'}] {n:20} {ms:>7}ms  {note}")
def run(sid,text,t=120):
    call({"cmd":"create_session","id":sid}); return call({"cmd":"send","session_id":sid,"text":text},t)
def evs(r): return (r.get("data") or {}).get("new_events") or []
def tools(evs_): return [e.get("call",{}).get("name") for e in evs_ if e.get("kind")=="tool_invoked"]

# R1 real add
r,ms=run("rt-add","请用工具计算 3 加 5 等于多少")
e=evs(r); tn=tools(e); tr=[x for x in e if x.get("kind")=="tool_result"]
sum_ok=any((x.get("output") or {}).get("sum")==8.0 for x in tr)
ft=(r.get("data") or {}).get("final_text","")
rec("R1_real_add","真模型:add(3+5=8)",{"text":"请用工具计算 3 加 5"},r,ms, r.get("ok")==True and "add" in tn and sum_ok, f"tools={tn} sum8={sum_ok} final={ft[:40]}")

# R2 real onto connector via real model
r,ms=run("rt-onto","帮我列出本体平台里有哪些对象类型")
e=evs(r); tn=tools(e); tr=[x for x in e if x.get("kind")=="tool_result"]
onto_ok = "onto_list_object_types" in tn and any((x.get("output") or {}).get("service")=="cmx-ontology" for x in tr)
ft=(r.get("data") or {}).get("final_text","")
rec("R2_real_onto","真模型:onto连接器→live",{"text":"列出对象类型"},r,ms, r.get("ok")==True and onto_ok, f"tools={tn} final={ft[:50]}")

summ={"base":BASE,"total":len(res),"passed":sum(1 for x in res if x['pass']),"failed":sum(1 for x in res if not x['pass']),"results":res}
open(OUT,"w").write(json.dumps(summ,ensure_ascii=False,indent=2))
print(f"\n真模型用例: {summ['passed']}/{summ['total']} PASS")
