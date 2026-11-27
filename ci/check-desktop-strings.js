// 데스크톱 앱 화면 문구 점검: app.js가 쓰는 모든 문구 키가 한국어·영어 둘 다에 있는지 확인한다.
// 사용: node ci/check-desktop-strings.js desktop/ui
const path=require("path"); const base=path.resolve(process.argv[2]); global.window={}; require(base+"/i18n.js");
const I=window.KM_I18N, src=require("fs").readFileSync(base+"/app.js","utf8");
const keys=new Set([...src.matchAll(/\bt\("([a-z0-9_.]+)"/g)].map(m=>m[1]).filter(k=>!k.endsWith(".")));
const ids=[...src.matchAll(/\{ id: "([a-z_]+)", menu:/g)].map(m=>m[1]); ids.forEach(i=>keys.add("act."+i));
["file","workspace","pane","agent","security","view","settings","help"].forEach(m=>keys.add("menu."+m));
["terminal","control","approvals","log"].forEach(v=>keys.add("view."+v));
["working","blocked","done","idle","unknown"].forEach(s=>keys.add("state."+s));
["none","restricted","container","wsl"].forEach(s=>keys.add("profile."+s));
["start","tasks","keys","words","about"].forEach(s=>keys.add("guide.tab."+s));
["1","2","3","4","5"].forEach(s=>keys.add("guide.start."+s));
["workspace","pane","agent","approval","protection","usage","log","mask","session"].forEach(s=>keys.add("guide.words."+s));
const tb=src.slice(src.indexOf("const TASKS = ["), src.indexOf("];", src.indexOf("const TASKS = ["))); for(const m of tb.matchAll(/\["([a-z]+)", (?:"[a-z_]+"|null)\]/g)){keys.add("task."+m[1]+".t");keys.add("task."+m[1]+".h");}
["agent_to_agent_prompt","agent_to_agent_run","agent_create","ask_timeout_secs","rate_per_min","agent_read"].forEach(k=>keys.add("set.d."+k));
["general","policy","sandbox","ledger","connection"].forEach(k=>keys.add("set.tab."+k));
["allow","ask","deny"].forEach(k=>keys.add("dec."+k));
for(let i=1;i<=4;i++){keys.add(`wz.${i}.t`);keys.add(`wz.${i}.b`);}
let bad=0; for(const k of keys){for(const l of ["ko","en"]) if(!(k in I[l])){console.log("MISSING",l,k);bad++;}}
console.log("checked",keys.size,"keys; missing",bad); process.exit(bad?1:0);
