// Build frontend first. Uses a local fake CPR iframe; never calls CPR or an upstream.
// CHROME_BIN=/path/to/chrome node scripts/check-ui.mjs
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { readFile, mkdtemp, rm, writeFile } from 'node:fs/promises'
import http from 'node:http'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { resolveTheme } from '../frontend/node_modules/@codex-proxy/ui/dist/theme/index.js'

assert(process.env.CHROME_BIN, 'Set CHROME_BIN to a headless Chrome binary')
const themes = Object.fromEntries(['light', 'dark'].map(mode => [mode, {
  ...resolveTheme(mode, 'relay-blue', '#3b82f6').tokens,
  '--cp-font-family': 'system-ui,sans-serif',
}]))
const dist = new URL('../frontend/dist/', import.meta.url)
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms))
const fixture = `
window.__requests = [];
const now = Date.now();
const H = 3600000;
const plans = ['plus','pro','team','plus','free'];
const reasons = {ok:'第一次带回就被接受（KlN 的判据：首次即接受为正常）',suspect:'换过一次票后才被接受，疑似降智',degraded:'连续 3 次带回最新的票，每次都被换成新票（KlN 的判据：全部刷新即降智）',unknown:'上游没有下发票，无法判定'};
const profile = i => i%7===0?'degraded':i%7===3?'suspect':i%11===5?'unknown':i%13===9?'empty':'ok';
const summary = (id, ago, level, status='done') => ({id, model:'gpt-6-luna', trigger:'auto', started_ms:now-ago, finished_ms:now-ago+40000, status, ...(status==='done'?{level}:{}), reason:status==='failed'?'上游连接超时':reasons[level]??'', detail:ago<8*H});
const accounts = Array.from({length:40},(_,i)=>{
  const state = profile(i);
  const abnormal = state==='degraded';
  return {
    id:'acct_'+i, name:'Account '+i, email:'test'+i+'@example.com', plan_type:plans[i%5], enabled:i!==17, credential_state:i===6?'expired':'ready',
    blocked: i===6?{code:'credential_state',message:'凭据已失效（CPR 标记为 expired，令牌可能已被上游作废），需要重新登录或等 CPR 刷新',since_ms:now-2*H,confirmed_ms:now-90*60000}
      : i===8?{code:'token_expired',message:'访问令牌已过期或即将过期。插件不刷新令牌（刷新令牌会轮换），等 CPR 自动刷新后再试',since_ms:now-12*60000,confirmed_ms:0} : null, supported:i!==38, token_expires_ms:null, running:null,
    __state: state,
    health: state==='empty'?{level:'unknown',model:'',candidate:null,streak:0,changed_ms:0,last_run_id:'',notification_pending:0,notification_error:null,notifications_dropped:0}
      : {level:state, model:'gpt-6-luna', candidate:i===0?'ok':i===1?'degraded':null, streak:i===0||i===1?2:0, changed_ms:now-(i+1)*3*H, last_run_id:'r'+i, notification_pending:i===0?1:0, notification_error:i===0?'TG 发送限流，稍后自动重试':null, notifications_dropped:0},
    schedule:{turnstate:{enabled:state!=='empty',model:abnormal?'':'gpt-6-astra',interval_minutes:[10,30,60][i%3]},modeltrace:{enabled:false,model:abnormal?'':'gpt-6-astra',interval_minutes:60}},
    last_attempt:null,
    latest:{turnstate: state==='empty'?null:summary('r'+i,(i%9+1)*60000,state==='unknown'?'unknown':state, state==='unknown'?'failed':'done'), modeltrace:null},
    strip:[], rate:{judged:0,ok:0}, rate_24h:{judged:0,ok:0}, rate_7d:{judged:0,ok:0},
  };
});
const removed = new Set(); window.__removeAccount = id => removed.add(id);
const bank = {commit:'a'.repeat(40),source:'github',models:['gpt-6-astra','gpt-6-luna'],checked_ms:now-2*H};
const ranges = {24:900000, 168:2*H, 720:6*H};
function rangeOf(hours){const slot=ranges[hours];const end=Math.floor((now+8*H)/slot)*slot-8*H+slot;return {hours,start_ms:end-hours*H,slot_ms:slot,slots:hours*H/slot}}
function cellsFor(a, range, index){
  if(a.__state==='empty'||!a.supported) return [];
  const cells=[];
  for(let i=0;i<range.slots;i++){
    if(i<range.slots/4||i%9===4) continue;
    const late=i>range.slots-10;
    const n=range.hours===24?1:range.hours===168?4:12;
    const c={i};
    if(a.__state==='ok') c.ok=n;
    else if(a.__state==='degraded') { if(late) c.degraded=n; else c.ok=n; }
    else if(a.__state==='suspect') { if(i%3===0) c.suspect=1, c.ok=n-1||0; else c.ok=n; }
    else c.failed=n;
    for(const k of Object.keys(c)) if(c[k]===0) delete c[k];
    cells.push(c);
  }
  return cells;
}
function detailOf(a){
  const runs=Array.from({length:24},(_,k)=>summary(a.id+'-'+k,(k*20+3)*60000,a.__state==='degraded'&&k<6?'degraded':a.__state==='suspect'&&k%4===0?'suspect':'ok', a.__state==='unknown'&&k%2?'failed':'done'));
  const day=Math.floor((now+8*H)/(24*H))*24*H-8*H;
  const daily=Array.from({length:30},(_,d)=>{const start_ms=day-(29-d)*24*H;if(d<6)return {start_ms};const o={start_ms,ok:a.__state==='degraded'&&d>26?20:120-(d%5)*7};if(a.__state==='degraded'&&d>26)o.degraded=100;if(a.__state==='suspect'&&d%4===0)o.suspect=9;if(d%6===0)o.failed=3;return o});
  return {...a, methods:{
    turnstate:{schedule:a.schedule.turnstate,last_attempt:null,health:a.health,runs,daily,raw_since:now-48*H},
    modeltrace:{schedule:a.schedule.modeltrace,last_attempt:null,health:null,runs:[{...summary(a.id+'-mt',26*H,'ok'),model:'gpt-6.1-sol',trigger:'manual',reason:'gpt-6.1-sol 的概率 82%，最像它'}],daily:daily.map(d=>({start_ms:d.start_ms})),raw_since:null},
  }};
}
const groups = {enabled:true,max_rules:8,plans:['plus','pro','team','free'],
  rules:[{id:'g1',name:'测票正常',color:'#22C55EFF',levels:['ok'],plans:[]},{id:'g2',group_id:'cpr-1',name:'PRO 异常',color:'#EF4444FF',levels:['suspect','degraded'],plans:['pro']}],
  status:{synced_ms:now-1800000,rules:{g1:{group_id:'x1',group_name:'测票正常'},g2:{group_id:'cpr-1',group_name:'PRO 异常'}},changes:[{account_id:'acct_7',from:'ok',to:'degraded',at_ms:now-1200000,added:['PRO 异常'],removed:['测票正常']}]}};
window.codexProxyPlugin = {version:2,get theme(){return document.documentElement.dataset.theme},request:async request=>{
 window.__requests.push(request);
 const q=new URLSearchParams(request.query);
 const reply=data=>({status:200,contentType:'application/json',body:new TextEncoder().encode(JSON.stringify(data)).buffer});
 if(request.path==='api/schedules/batch'){
   const body=JSON.parse(request.body);
   for(const id of body.account_ids){const account=accounts.find(a=>a.id===id);Object.assign(account.schedule[body.method],Object.fromEntries(Object.entries(body).filter(([k])=>!['method','account_ids'].includes(k))));}
   return reply({updated:body.account_ids.length});
 }
 if(request.path==='api/overview'){
   const range=rangeOf(Number(q.get('hours')||24));
   const out=accounts.filter(a=>!removed.has(a.id)).map((a,index)=>{const strip=cellsFor(a,range,index);const sum=k=>strip.reduce((n,c)=>n+(c[k]??0),0);const judged=sum('ok')+sum('suspect')+sum('degraded');const {__state,...rest}=a;return {...rest,strip,rate:{judged,ok:sum('ok')}}});
   return reply({now_ms:now,range,config:{probe_enabled:true,min_interval_minutes:10,fingerprint_sync_hours:6,telegram_enabled:true,telegram_configured:true},bank,accounts:out});
 }
 const data = request.path === 'api/telegram/test' ? {sent:true}
 : request.path === 'api/account' ? (()=>{const {__state,...a}=detailOf(accounts.find(a=>a.id===q.get('id')));return {account:{...a,methods:detailOf(accounts.find(x=>x.id===q.get('id'))).methods},config:{probe_enabled:true}}})()
 : request.path === 'api/runs' ? (Number(q.get('from'))<now-48*H ? {runs:[],counts:{ok:9,degraded:3},complete:false} : {runs:[{id:'r1',model:'gpt-6-luna',trigger:'auto',started_ms:Number(q.get('from'))+1,finished_ms:0,status:'done',level:'degraded',reason:'模拟判定',detail:true}],counts:{degraded:1},complete:true})
 : request.path === 'api/models' ? {bank:[{id:'gpt-6-astra',display_name:'GPT',family:'GPT'}],cpr:['gpt-6-astra','gpt-6-luna'],aliases:{}}
 : request.path === 'api/plan' ? {blockers:[],request:{headers:{'User-Agent':'CPR identity'},body:'request details '.repeat(200)}}
 : request.path === 'api/groups' ? groups
 : request.path === 'api/groups/cpr' ? {groups:[{id:'cpr-1',name:'PRO 异常',color:'#EF4444FF',enabled:true,plugin:false},{id:'cpr-2',name:'默认',color:'#3B82F6FF',enabled:true,plugin:false}]}
 : (()=>{throw Error('Unexpected request '+request.path)})();
 return reply(data);
}};
addEventListener('load',()=>new ResizeObserver(()=>parent.postMessage({height:Math.ceil(document.body.getBoundingClientRect().height)},'*')).observe(document.body),{once:true});
`;
const server = http.createServer(async (req, res) => {
  try {
    const path = new URL(req.url, 'http://localhost').pathname
    if (path === '/') {
      res.setHeader('Content-Type', 'text/html')
      res.end(`<style>body{margin:0}header{height:80px}iframe{display:block;width:100%;border:0}</style><header>CPR mock host</header><iframe id="plugin" sandbox="allow-scripts allow-same-origin" src="/plugin"></iframe><script>
const frame=document.querySelector('iframe'); const height=innerHeight-100; frame.style.height=height+'px';
addEventListener('message',e=>{if(e.source===frame.contentWindow&&typeof e.data?.height==='number')frame.style.height=Math.max(height,e.data.height)+'px'});
</script>`)
    }
    else if (path === '/plugin') {
      const html = await readFile(new URL('index.html', dist), 'utf8')
      res.setHeader('Content-Type', 'text/html')
      // Like CPR, parse namespace metadata before installing the bridge. Do not inject
      // cp tokens directly: that used to hide the new host's default z namespace.
      const prefixMeta = html.match(/<meta\s+name="codex-proxy-ui-theme-prefix"[^>]*>/)?.[0] ?? ''
      const viewportProperty = themeProtocol === 'modern' ? '--plugin-viewport-height' : '--cp-plugin-viewport-height'
      const themeBridge = `
        const themeProtocol=${JSON.stringify(themeProtocol)};
        const themePayloads=${JSON.stringify(themes)};
        const themePrefix=document.querySelector('meta[name="codex-proxy-ui-theme-prefix"]')?.content ?? 'z';
        window.__setHostTheme=(name)=>{
          const root=document.documentElement;
          root.dataset.theme=name;root.style.colorScheme=name;
          for(const [legacyName,value] of Object.entries(themePayloads[name])){
            const token=legacyName.replace(/^--cp-/, '');
            root.style.setProperty(themeProtocol==='modern'?'--'+themePrefix+'-'+token:legacyName,value);
          }
          window.dispatchEvent(new CustomEvent('codex-proxy-themechange',{detail:{theme:name}}));
        };
        document.documentElement.style.setProperty(${JSON.stringify(viewportProperty)},${JSON.stringify(`${viewportHeight}px`)});
        window.__setHostTheme(${JSON.stringify(themeMode)});
      `
      res.end(html.replace(prefixMeta, '').replace('<head>', `<head>${prefixMeta}<style>html{overflow:hidden}body{display:flow-root;box-sizing:border-box;min-height:var(${viewportProperty})}</style><script>${themeBridge}${fixture}</script>`))
    }
    else if (path === '/app.js' || path === '/app.css') {
      res.setHeader('Content-Type', path.endsWith('.js') ? 'text/javascript' : 'text/css')
      res.end(await readFile(new URL(path.slice(1), dist)))
    }
    else { res.writeHead(404); res.end() }
  }
  catch (error) { res.writeHead(500); res.end(String(error)) }
})
let viewportHeight = 700
let themeMode = 'light'
let themeProtocol = 'legacy'
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
const base = `http://127.0.0.1:${server.address().port}`
const profile = await mkdtemp(join(tmpdir(), 'cpr-ui-'))
const chrome = spawn(process.env.CHROME_BIN, ['--headless=new','--no-sandbox','--remote-debugging-port=0',`--user-data-dir=${profile}`,'about:blank'], {stdio:'ignore'})
let ws
try {
  let port
  for (let i=0;i<100&&!port;i++) {
    try { port=(await readFile(join(profile,'DevToolsActivePort'),'utf8')).split('\n')[0] } catch { await sleep(100) }
  }
  assert(port, 'Chrome did not start')
  const targets=await (await fetch(`http://127.0.0.1:${port}/json`)).json()
  ws=new WebSocket(targets.find(t=>t.type==='page').webSocketDebuggerUrl)
  await new Promise(resolve=>ws.addEventListener('open',resolve,{once:true}))
  let sequence=0
  const pending=new Map(), contexts=new Map(), errors=[]
  ws.onmessage=({data})=>{
    const message=JSON.parse(data)
    if(message.id) { const p=pending.get(message.id);if(!p)return;pending.delete(message.id);clearTimeout(p.timer);message.error?p.reject(Error(JSON.stringify(message.error))):p.resolve(message.result);return }
    if(message.method==='Runtime.executionContextCreated'&&message.params.context.auxData?.isDefault)contexts.set(message.params.context.auxData.frameId,message.params.context.id)
    if(message.method==='Runtime.executionContextsCleared')contexts.clear()
    if(message.method==='Runtime.exceptionThrown')errors.push(message.params.exceptionDetails)
  }
  const send=(method,params={})=>new Promise((resolve,reject)=>{const id=++sequence;pending.set(id,{resolve,reject,timer:setTimeout(()=>{pending.delete(id);reject(Error('CDP timed out: '+method))},15000)});ws.send(JSON.stringify({id,method,params}))})
  const evaluate=async(expression,contextId)=>{
    const reply=await send('Runtime.evaluate',{expression,contextId,awaitPromise:true,returnByValue:true})
    assert(!reply.exceptionDetails,JSON.stringify(reply.exceptionDetails))
    return reply.result.value
  }
  await send('Page.enable');await send('Runtime.enable')
  const matrix = ['legacy', 'modern'].flatMap(protocol => ['light', 'dark'].flatMap(mode => [[1280,900],[390,844]].map(([width,height]) => ({ protocol, mode, width, height }))))
  for (const {protocol, mode, width, height} of matrix) {
    themeMode=mode;themeProtocol=protocol;
    viewportHeight=height-100
    await send('Emulation.setDeviceMetricsOverride',{width,height,deviceScaleFactor:1,mobile:false})
    await send('Page.navigate',{url:base})
    let context
    for(let i=0;i<100&&!context;i++) {
      const tree=await send('Page.getFrameTree')
      const id=tree.frameTree.childFrames?.[0]?.frame.id
      const candidate=contexts.get(id)
      if(candidate&&await evaluate('document.querySelectorAll("#app button").length>40',candidate))context=candidate
      if(!context)await sleep(100)
    }
    assert(context, 'Plugin did not render: '+JSON.stringify({errors,frames:await send('Page.getFrameTree'),contexts:[...contexts]}))
    await sleep(300)
    const assertTheme = async expected => {
      const style = await evaluate(`(()=>{
        const root=getComputedStyle(document.documentElement);
        const summary=getComputedStyle(document.querySelector('.filter-chip:not(.active)'));
        const list=getComputedStyle(document.querySelector('.monitor-list'));
        const input=getComputedStyle(document.querySelector('.monitor-search > span'));
        const button=getComputedStyle(document.querySelector('.monitor-actions button'));
        const empty=getComputedStyle(document.querySelector('.probe-cell[data-state=empty]'));
        const resolve=(token)=>{const el=document.createElement('i');el.style.color='var('+token+')';document.body.append(el);const color=getComputedStyle(el).color;el.remove();return color};
        return {theme:window.codexProxyPlugin.theme,layout:root.backgroundColor,expectedLayout:resolve('--cp-color-bg-layout'),
          surface:summary.backgroundColor,expectedSurface:resolve('--cp-color-bg-container'),border:summary.borderTopColor,borderWidth:summary.borderTopWidth,
          list:list.backgroundColor,input:input.backgroundColor,button:button.backgroundColor,empty:empty.backgroundColor,
          text:root.color,expectedText:resolve('--cp-color-text'),appHeight:document.querySelector('#app').clientHeight,
          token:root.getPropertyValue('--cp-color-bg-container').trim()};
      })()`,context);
      assert.equal(style.theme,expected);
      assert(style.token,`Missing theme tokens: ${protocol}/${expected}`);
      assert.equal(style.layout,style.expectedLayout,JSON.stringify(style));
      assert.equal(style.text,style.expectedText,JSON.stringify(style));
      assert.equal(style.surface,style.expectedSurface,JSON.stringify(style));
      assert.equal(style.list,style.expectedSurface,JSON.stringify(style));
      assert.equal(style.borderWidth,'1px',JSON.stringify(style));
      assert(!['rgba(0, 0, 0, 0)','transparent'].includes(style.border),JSON.stringify(style));
      for(const field of ['input','button','empty']) assert(!['rgba(0, 0, 0, 0)','transparent'].includes(style[field]),`Missing ${field} styling: ${JSON.stringify(style)}`);
      assert.equal(style.appHeight,viewportHeight,`Viewport mismatch: ${protocol}/${expected}`);
      return style;
    };
    const shot=async name=>{if(!process.env.SHOTS)return;const image=await send('Page.captureScreenshot',{format:'png'});await writeFile(join(process.env.SHOTS,`${name}-${protocol}-${mode}-${width}.png`),Buffer.from(image.data,'base64'))};
    const original=await assertTheme(mode);
    const other=mode==='light'?'dark':'light';
    await evaluate(`window.__setHostTheme('${other}')`,context);
    await sleep(100);
    const switched=await assertTheme(other);
    assert.notEqual(original.surface,switched.surface,'Live theme change must update card surfaces');
    await evaluate(`window.__setHostTheme('${mode}')`,context);
    await sleep(100);
    await assertTheme(mode);
    console.log(`PASS ${protocol} ${mode} ${width}x${height}: surfaces, controls, empty cells, viewport and live theme switching`);
    for (const last of [false,true]) {
      await evaluate(`(()=>{const app=document.querySelector('#app');app.scrollTop=${last?'app.scrollHeight':'0'};const buttons=[...app.querySelectorAll('button')].filter(b=>b.textContent.trim()==='探测');buttons[${last?'buttons.length-1':'0'}].click()})()`,context)
      await sleep(300)
      const rect=await evaluate(`(()=>{const panel=document.querySelector('[role="dialog"]');const r=panel?.getBoundingClientRect();return {top:r?.top,bottom:r?.bottom,center:r?(r.top+r.bottom)/2:null,height:innerHeight,appHeight:document.querySelector('#app').clientHeight,bodyHeight:document.body.getBoundingClientRect().height,overflow:document.documentElement.scrollWidth>innerWidth,started:window.__requests.some(r=>r.path==='api/probe')}})()`,context)
      assert(rect.center!==null, 'Dialog missing')
      assert(rect.top>=0&&rect.bottom<=viewportHeight, `Dialog outside visible area: ${JSON.stringify(rect)}`)
      assert(Math.abs(rect.center-viewportHeight/2)<3, `Dialog not centered: ${JSON.stringify(rect)}`)
      assert(!rect.overflow, 'Horizontal overflow')
      assert(!rect.started, 'Opening the dialog started a probe')
      if(!last){
        assert.equal(await evaluate("JSON.parse(window.__requests.filter(r=>r.path==='api/plan').at(-1).body).model",context),'gpt-6-luna','Ticket default model');
        await evaluate("[...document.querySelectorAll('[role=dialog] [role=radio]')].find(e=>e.textContent.trim()==='ModelTrace').click()",context);
        await sleep(200);
        assert.equal(await evaluate("JSON.parse(window.__requests.filter(r=>r.path==='api/plan').at(-1).body).model",context),'gpt-6.1-sol','ModelTrace default on method switch');
        await evaluate("[...document.querySelectorAll('[role=dialog] [role=radio]')].find(e=>e.textContent.trim()==='测票').click()",context);
        await sleep(200);
        assert.equal(await evaluate("JSON.parse(window.__requests.filter(r=>r.path==='api/plan').at(-1).body).model",context),'gpt-6-luna','Ticket choice retained on switch back');
      } else {
        assert.equal(await evaluate("JSON.parse(window.__requests.filter(r=>r.path==='api/plan').at(-1).body).model",context),'gpt-6-astra','Configured account model must remain selected');
      }

      await evaluate(`(()=>{const panel=document.querySelector('[role="dialog"]');[...panel.querySelectorAll('button')].find(b=>b.textContent.trim()==='关闭').click()})()`,context)
      await sleep(250)
      console.log(`PASS ${width}x${height}, ${last?'last':'first'} account: dialog centered, visible, no probe sent`)
    }
    await evaluate("document.querySelector('#app').scrollTop=0",context);
    await sleep(150);
    const cells=await evaluate("[...document.querySelectorAll('.probe-cell')].slice(0,10).map(e=>({w:e.getBoundingClientRect().width,h:e.getBoundingClientRect().height,color:getComputedStyle(e).backgroundColor}))",context);
    assert(cells.length===10&&cells.every(c=>c.w>=5&&c.w<=13&&c.h>=20),'Cells must have visible, consistent dimensions: '+JSON.stringify(cells));
    await evaluate("document.querySelector('.probe-cell[data-state=empty]').click()",context);
    await sleep(200);
    assert(await evaluate("[...document.querySelectorAll('[role=dialog]')].some(e=>e.textContent.includes('这段时间没有发起探测，不计入正常率'))",context),'Missing no-probe explanation');
    await evaluate("window.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape'}))",context);
    await sleep(200);
    await evaluate("document.querySelector('.probe-cell:not([data-state=empty])').click()",context);
    await sleep(300);
    await shot('popover');
    assert(await evaluate("window.__requests.some(r=>r.path==='api/runs')",context),'Opening a probed cell must load its records');
    assert(await evaluate("[...document.querySelectorAll('[role=dialog]')].some(e=>e.textContent.includes('模拟判定'))",context),'Loaded records missing from the cell popover');
    await evaluate("window.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape'}))",context);
    await sleep(200);
    await shot('list');
    for(const [label,range] of [['7 天',168],['30 天',720],['24 小时',24]]) {
      await evaluate(`([...document.querySelectorAll('button')].find(b=>b.textContent.trim()==='${label}')).click()`,context);
      await sleep(300);
      assert(await evaluate(`window.__requests.some(r=>r.path==='api/overview'&&new URLSearchParams(r.query).get('hours')==='${range}')`,context),'Time range was not requested');
    }
    console.log(`PASS ${width}x${height} time filters request complete 24h / 7d / 30d ranges`);
    await evaluate("[...document.querySelectorAll('button')].find(b=>b.textContent.trim()==='设置').click()",context);
    await sleep(300);
    assert(await evaluate("window.__requests.some(r=>r.path==='api/groups')&&document.body.textContent.includes('PRO 异常')",context),'Auto-grouping settings missing');
    assert(await evaluate("document.body.textContent.includes('test7@example.com · 正常 → 降智')",context),'Group changes must name the account, not its internal ID');
    assert(await evaluate("[...document.querySelectorAll('button')].some(b=>b.textContent.trim()==='已保存'&&b.disabled)",context),'Unchanged group settings must not offer saving');
    await shot('settings-groups');
    await evaluate("[...document.querySelectorAll('[role=radio]')].find(e=>e.textContent.trim()==='通知与运行').click()",context);
    await sleep(200);
    await evaluate("[...document.querySelectorAll('button')].find(b=>b.textContent.trim()==='发送测试通知').click()",context);
    await sleep(150);
    assert(await evaluate("window.__requests.some(r=>r.path==='api/telegram/test')&&document.body.textContent.includes('TG 测试通知已发送')",context),'Telegram test action missing');
    await shot('settings-notify');
    await evaluate("[...document.querySelectorAll('button')].find(b=>b.textContent.trim()==='返回列表').click()",context);
    await sleep(300);
    await evaluate("document.querySelector('#app').scrollTop=0;document.querySelector('[data-account=acct_0] input[type=checkbox]').click();document.querySelector('[data-account=acct_1] input[type=checkbox]').click();",context);
    await sleep(100);
    await evaluate("[...document.querySelectorAll('button')].find(b=>b.textContent.trim()==='批量配置').click()",context);
    await sleep(200);
    await evaluate("[...document.querySelectorAll('[role=dialog] [role=radio]')].find(e=>e.textContent.trim()==='开启').click()",context);
    await evaluate("document.querySelector('[role=dialog] [role=combobox]').click()",context);
    await sleep(200);
    await evaluate("[...document.querySelectorAll('[role=option]')].find(e=>e.textContent.trim()==='gpt-6-luna').click()",context);
    await evaluate("[...document.querySelectorAll('[role=dialog] button')].find(b=>b.textContent.includes('应用到 2 个账号')).click()",context);
    await sleep(300);
    const batch=await evaluate("JSON.parse(window.__requests.find(r=>r.path==='api/schedules/batch').body)",context);
    assert.deepEqual(batch,{account_ids:['acct_0','acct_1'],method:'turnstate',enabled:true,model:'gpt-6-luna'});
    assert(!await evaluate("window.__requests.some(r=>r.path==='api/probe')",context),'Configuration must never start a probe');
    await sleep(200);
    await evaluate("[...document.querySelectorAll('.selection-bar button')].find(b=>b.textContent.trim()==='批量探测').click()",context);
    await sleep(300);
    assert.equal(await evaluate("document.querySelectorAll('[role=dialog] .batch-item').length",context),2,'Batch probe lists the selected accounts');
    assert(!await evaluate("window.__requests.some(r=>r.path==='api/probe')",context),'Opening batch probe must not start probes');
    await shot('batch-probe');
    await evaluate("[...document.querySelectorAll('[role=dialog] button')].find(b=>b.textContent.trim()==='关闭').click()",context);
    await sleep(250);
    await evaluate("[...document.querySelectorAll('.filter-chip')].find(b=>b.textContent.includes('需要关注')).click()",context);
    await sleep(150);
    assert(await evaluate("[...document.querySelectorAll('.monitor-row')].filter(r=>r.offsetParent).every(r=>['degraded','suspect'].includes(r.dataset.state)||r.dataset.blocked)",context),'Attention filter shows only abnormal or blocked accounts');
    assert(await evaluate("[...document.querySelectorAll('.monitor-row')].some(r=>r.offsetParent&&r.dataset.account==='acct_6')",context),'A confirmed block needs attention');
    assert(await evaluate("document.querySelector('.selection-bar').textContent.includes('1 个不在当前筛选里')",context),'Hidden selected accounts must be called out');
    await shot('filtered');
    await evaluate("document.querySelector('[aria-label=\"查看 test0@example.com\"]').click()",context);
    await sleep(400);
    assert(await evaluate("document.body.textContent.includes('自动探测')&&document.querySelectorAll('[role=combobox]').length>=1",context),'Per-account configuration missing');
    assert(await evaluate("document.body.textContent.includes('测票当前判定')&&document.body.textContent.includes('2 / 3 次正常')&&document.body.textContent.includes('TG 待发送 1 条')",context),'Health streak or notification status missing');
    assert(await evaluate("document.querySelectorAll('.trend-chart rect').length>=30",context),'Trend chart missing');
    await shot('detail');
    await evaluate("[...document.querySelectorAll('button')].find(b=>b.textContent.trim()==='返回列表').click()",context);
    await sleep(300);
    assert(await evaluate("document.querySelector('.filter-chip.active').textContent.includes('需要关注')&&document.querySelector('.selection-bar').textContent.includes('已选 2 个')",context),'Returning to the list keeps its filter and selection');
    await evaluate("[...document.querySelectorAll('.filter-chip')].find(b=>b.textContent.includes('全部')).click();[...document.querySelectorAll('.selection-bar button')].find(b=>b.textContent.trim()==='清空').click()",context);
    await sleep(200);
    // 受阻不改判定，但排在疑似之后、未判定之前；有单独的筛选
    const order=await evaluate("[...document.querySelectorAll('.monitor-row')].map(r=>({id:r.dataset.account,state:r.dataset.state,blocked:!!r.dataset.blocked}))",context);
    const at=order.findIndex(r=>r.id==='acct_6');
    assert(order[at].state==='ok'&&order[at].blocked,'Blocked account keeps its verdict');
    assert(at>order.findLastIndex(r=>r.state==='suspect')&&at<order.findIndex(r=>r.state==='unknown'),'Confirmed block sorts between suspect and unknown: '+JSON.stringify(order.slice(0,16)));
    assert(!order.find(r=>r.id==='acct_8').blocked,'An unconfirmed block does not change sorting');
    assert(await evaluate("document.querySelector('[data-account=acct_6] .blocked-pill').textContent.includes('凭据失效')&&document.querySelector('[data-account=acct_8] .blocked-pill').textContent.includes('令牌过期')",context),'Blocked pills missing');
    await evaluate("[...document.querySelectorAll('.filter-chip')].find(b=>b.textContent.includes('探测受阻')).click()",context);
    await sleep(150);
    assert.deepEqual(await evaluate("[...document.querySelectorAll('.monitor-row')].filter(r=>r.offsetParent).map(r=>r.dataset.account)",context),['acct_6'],'Blocked filter');
    await shot('blocked');
    await evaluate("document.querySelector('[aria-label=\"查看 test6@example.com\"]').click()",context);
    await sleep(400);
    assert(await evaluate("document.querySelector('.blocked-banner')?.textContent.includes('探测受阻')&&document.body.textContent.includes('这不是降智判定')",context),'Detail page shows the block');
    await evaluate("[...document.querySelectorAll('button')].find(b=>b.textContent.trim()==='返回列表').click()",context);
    await sleep(300);
    await evaluate("[...document.querySelectorAll('.filter-chip')].find(b=>b.textContent.includes('全部')).click()",context);
    await sleep(150);
    // 选中的账号在 CPR 里被删掉：刷新后要从选中里去掉，否则批量配置整批被拒
    await evaluate("document.querySelector('[data-account=acct_2] input[type=checkbox]').click();document.querySelector('[data-account=acct_4] input[type=checkbox]').click()",context);
    await sleep(100);
    await evaluate("window.__removeAccount('acct_2');[...document.querySelectorAll('button')].find(b=>b.getAttribute('aria-label')==='刷新').click()",context);
    await sleep(300);
    assert(await evaluate("document.querySelector('.selection-bar').textContent.includes('已选 1 个')",context),'Refreshing must drop accounts that no longer exist');
    await evaluate("[...document.querySelectorAll('.selection-bar button')].find(b=>b.textContent.trim()==='清空').click()",context);
    await sleep(150);
    console.log(`PASS ${width}x${height} method defaults, per-account controls and batch model configuration, no probe sent`);


  }
  assert.equal(errors.length,0,JSON.stringify(errors))
}
finally {
  ws?.close();chrome.kill('SIGKILL');await new Promise(resolve=>chrome.once('exit',resolve));server.close();await rm(profile,{recursive:true,force:true,maxRetries:5,retryDelay:100})
}
