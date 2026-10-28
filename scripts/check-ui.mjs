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
const tokens = Object.entries(resolveTheme('light', 'relay-blue', '#3b82f6').tokens).map(([key, value]) => `${key}:${value}`).join(';')
const dist = new URL('../frontend/dist/', import.meta.url)
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms))
const fixture = `
window.__requests = [];
const method = () => ({schedule:{enabled:false,model:'gpt-6-astra',interval_minutes:60},last_attempt:null,latest:null,slots:Array.from({length:96},(_,i)=>({start_ms:Date.now()-(96-i)*900000,end_ms:Date.now()-(95-i)*900000,level:null,runs:[]})),rate:{judged:0,ok:0},rate_24h:{judged:0,ok:0},rate_7d:{judged:0,ok:0}});
const accounts = Array.from({length:40},(_,i)=>({id:'acct_'+i,name:'Account '+i,email:'test'+i+'@example.com',plan_type:'plus',enabled:true,credential_state:'ready',supported:true,token_expires_ms:null,running:null,methods:{turnstate:method(),modeltrace:method()}}));
const bank = {commit:'a'.repeat(40),source:'embedded',models:['gpt-6-astra'],checked_ms:0};
window.codexProxyPlugin = {version:2,theme:'light',request:async request=>{
 window.__requests.push(request);
 const hours=Number(new URLSearchParams(request.query).get('hours')||24);
 const count=hours===168?84:hours===720?120:96;
 for(const [index,account] of accounts.entries())for(const m of Object.values(account.methods)){
   m.slots=Array.from({length:count},(_,i)=>{
     const start=Date.now()-(count-i)*hours*3600000/count;
     const active=index%5!==0&&i>count/3&&i%7!==0;
     const level=index%5===1?'ok':index%5===2&&i>count-9?'degraded':index%5===3?'unknown':'ok';
     return {start_ms:start,end_ms:start+hours*3600000/count,level:active?level:null,runs:active?[{id:index+'-'+i,started_ms:start+1,model:'gpt-6-astra',status:level==='unknown'?'failed':'done',level,reason:level==='unknown'?'上游连接失败':'模拟判定',detail:true}]:[]}
   });
   m.rate={judged:index%5===0||index%5===3?0:30,ok:index%5===2?27:30};
 }

 const data = request.path === 'api/overview' ? {now_ms:Date.now(),range_hours:hours,config:{probe_enabled:false,min_interval_minutes:30,fingerprint_sync_hours:6},bank,accounts}
 : request.path === 'api/models' ? {bank:[{id:'gpt-6-astra',display_name:'GPT',family:'GPT'}],cpr:['gpt-6-astra'],aliases:{}}
 : request.path === 'api/plan' ? {blockers:[],request:{headers:{'User-Agent':'CPR identity'},body:'request details '.repeat(200)}}
 : (()=>{throw Error('Unexpected request '+request.path)})();
 return {status:200,contentType:'application/json',body:new TextEncoder().encode(JSON.stringify(data)).buffer};
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
      res.end(html.replace('<head>', `<head><style>:root{${tokens};--cp-font-family:system-ui,sans-serif;--cp-plugin-viewport-height:${viewportHeight}px}html{overflow:hidden}body{display:flow-root;box-sizing:border-box;min-height:var(--cp-plugin-viewport-height)}</style><script>${fixture}</script>`))
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
  for (const [width,height] of [[1280,900],[390,844]]) {
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
    for (const last of [false,true]) {
      await evaluate(`(()=>{const app=document.querySelector('#app');app.scrollTop=${last?'app.scrollHeight':'0'};const buttons=[...app.querySelectorAll('button')].filter(b=>b.textContent.trim()==='探测');buttons[${last?'buttons.length-1':'0'}].click()})()`,context)
      await sleep(300)
      const rect=await evaluate(`(()=>{const panel=document.querySelector('[role="dialog"]');const r=panel?.getBoundingClientRect();return {top:r?.top,bottom:r?.bottom,center:r?(r.top+r.bottom)/2:null,height:innerHeight,appHeight:document.querySelector('#app').clientHeight,bodyHeight:document.body.getBoundingClientRect().height,overflow:document.documentElement.scrollWidth>innerWidth,started:window.__requests.some(r=>r.path==='api/probe')}})()`,context)
      assert(rect.center!==null, 'Dialog missing')
      assert(rect.top>=0&&rect.bottom<=viewportHeight, `Dialog outside visible area: ${JSON.stringify(rect)}`)
      assert(Math.abs(rect.center-viewportHeight/2)<3, `Dialog not centered: ${JSON.stringify(rect)}`)
      assert(!rect.overflow, 'Horizontal overflow')
      assert(!rect.started, 'Opening the dialog started a probe')
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
    const screenshot=await send('Page.captureScreenshot',{format:'png'});
    await writeFile(join(tmpdir(),`cpr-ui-${width}.png`),Buffer.from(screenshot.data,'base64'));
    for(const [label,range] of [['7 天',168],['30 天',720],['24 小时',24]]) {
      await evaluate(`([...document.querySelectorAll('button')].find(b=>b.textContent.trim()==='${label}')).click()`,context);
      await sleep(300);
      assert(await evaluate(`window.__requests.some(r=>r.path==='api/overview'&&new URLSearchParams(r.query).get('hours')==='${range}')`,context),'Time range was not requested');
    }
    console.log(`PASS ${width}x${height} time filters request complete 24h / 7d / 30d ranges`);

  }
  assert.equal(errors.length,0,JSON.stringify(errors))
}
finally {
  ws?.close();chrome.kill('SIGKILL');await new Promise(resolve=>chrome.once('exit',resolve));server.close();await rm(profile,{recursive:true,force:true,maxRetries:5,retryDelay:100})
}
