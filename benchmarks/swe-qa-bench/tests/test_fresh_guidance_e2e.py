import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

from zg_bench.fresh_guidance_e2e import matching_guidance
from zg_bench.relationship_diagnostic import DIRECTIVE, READY

ROOT = Path(__file__).resolve().parents[1]


class FreshGuidanceTests(unittest.TestCase):
    def test_directive_is_inside_agents_block_and_matches_mcp(self):
        frozen = json.loads((ROOT / "fixtures/caller-callee-guidance-v2.json").read_text())["groups"]["relationships"]
        agents, instructions = matching_guidance(frozen)
        self.assertIn(DIRECTIVE, agents.split("<!-- ZVEC_GREP_END -->")[0])
        self.assertIn(READY, instructions)
        self.assertIn(DIRECTIVE.replace("zvec_grep_zvec_grep_", "zvec_grep_"), instructions)
        self.assertNotIn(DIRECTIVE, frozen["guidance"])

    def test_real_http_chain_changes_only_mcp_instructions_and_gates_fresh_requests(self):
        script = r'''
import assert from 'node:assert/strict';
import http from 'node:http';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {readFileSync,writeFileSync} from 'node:fs';
import {createInterface} from 'node:readline';
const {startProxy}=await import(process.env.PROXY);
const seen=[];
const tool={name:'zvec_grep_callees',description:'unchanged',inputSchema:{type:'object'}};
const mcp=http.createServer(async(req,res)=>{
 const chunks=[];for await(const c of req)chunks.push(c);
 const rpc=JSON.parse(Buffer.concat(chunks));seen.push(rpc);
 const result=rpc.method==='initialize'?{instructions:'old',protocolVersion:'2025-03-26'}:rpc.method==='tools/list'?{tools:[tool]}:{content:[{type:'text',text:'real mocked result'}]};
 res.writeHead(200,{'Content-Type':'text/event-stream','Mcp-Session-Id':'session'});
 res.end('data: \n\ndata: '+JSON.stringify({jsonrpc:'2.0',id:rpc.id,result})+'\n\n');
});
await new Promise(r=>mcp.listen(0,'127.0.0.1',r));
const proxy=await startProxy('http://127.0.0.1:'+mcp.address().port+'/mcp',process.env.LOG,{instructions:'new guidance'});
async function rpc(method,params={}){
 const r=await fetch(proxy.url,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({jsonrpc:'2.0',id:1,method,params})});
 return JSON.parse((await r.text()).split('\n').find(l=>l.startsWith('data: {')).slice(6)).result;
}
assert.equal((await rpc('initialize')).instructions,'new guidance');
assert.deepEqual((await rpc('tools/list')).tools,[tool]);
const args={root:'/app',symbol:'Coordinates.update',limit:20};
await rpc('tools/call',{name:tool.name,arguments:args});
assert.deepEqual(seen.at(-1).params.arguments,args);
const forwarded=[];
const api=http.createServer(async(req,res)=>{
 const chunks=[];for await(const c of req)chunks.push(c);forwarded.push(Buffer.concat(chunks).toString());
 res.writeHead(200,{'Content-Type':'text/event-stream'});
 res.end('data: {"id":"mock","choices":[{"index":0,"delta":{"content":"ok"},"finish_reason":"stop"}]}\n\ndata: [DONE]\n\n');
});
await new Promise(r=>api.listen(0,'127.0.0.1',r));
const contract={guidance:'new agents',instructions:'new guidance',tool_names:['zvec_grep_zvec_grep_callees']};
writeFileSync(process.env.CONTRACT,JSON.stringify(contract));
const child=spawn('node',[process.env.RECORDER,'http://127.0.0.1:'+api.address().port+'/v1',process.env.REQUESTS,process.env.CLOSED,process.env.CONTRACT],{stdio:['ignore','pipe','pipe']});
const lines=createInterface({input:child.stdout});
const control=JSON.parse((await once(lines,'line'))[0]);
const body={model:'glm-5.2',messages:[{role:'system',content:'new agents\n<mcp_instructions><server name="zvec_grep">new guidance</server></mcp_instructions>'},{role:'user',content:'question'}],tools:[{type:'function',function:{name:contract.tool_names[0],parameters:{type:'object'}}}],tool_choice:'auto',stream:true};
async function send(b){const r=await fetch(control.baseURL+'/chat/completions',{method:'POST',headers:{'Content-Type':'application/json',Authorization:'Bearer fake-contract-key'},body:JSON.stringify(b)});await r.text();return r.status;}
const history=structuredClone(body);history.messages.push({role:'assistant',content:'old history'});
assert.equal(await send(history),400);
const missing=structuredClone(body);missing.messages[0].content='missing agents';
assert.equal(await send(missing),400);
assert.equal(forwarded.length,0);
assert.equal(await send(body),200);
assert.deepEqual(forwarded,[JSON.stringify(body)]);
const ended=once(child,'exit');child.kill('SIGTERM');assert.equal((await ended)[0],0);
const records=readFileSync(process.env.REQUESTS,'utf8');
assert(!records.includes('fake-contract-key'));
const events=records.trim().split('\n').map(JSON.parse);
assert.equal(events.filter(e=>e.type==='guidance_rejected').length,2);
assert.equal(events.filter(e=>e.type==='guidance_verified'&&e.first_task_request).length,1);
await proxy.close();await new Promise(r=>mcp.close(r));await new Promise(r=>api.close(r));
'''
        with tempfile.TemporaryDirectory() as directory:
            d = Path(directory)
            env = {**os.environ, "PROXY": str(ROOT / "zg_bench/fixed_search_proxy.mjs"), "RECORDER": str(ROOT / "zg_bench/model_request_recorder.mjs"),
                   "LOG": str(d / "mcp.jsonl"), "CONTRACT": str(d / "contract.json"), "REQUESTS": str(d / "requests.jsonl"), "CLOSED": str(d / "closed")}
            result = subprocess.run(["node", "--input-type=module", "--eval", script], env=env, capture_output=True, text=True, timeout=30)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
