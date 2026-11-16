"""Execution controls must constrain search without changing relationship calls."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import AsyncMock, patch

from zg_bench.agents.zvec_opencode import ZvecOpenCode
from zg_bench.tool_experiment import ROOT, audit_fixed_search, experiment_tasks, validate_fixed_surface


class FixedSearchTests(unittest.TestCase):
    def test_real_http_proxy_preserves_mcp_and_records_requested_and_executed_arguments(self):
        script = r'''
import http from 'node:http';
import assert from 'node:assert/strict';
const {startProxy} = await import(process.env.PROXY);
const seen = [];
let sse = false;
const tool = {name:'zvec_grep_search',description:'set preview: "full" to return each complete stored entity without changing retrieval or ranking',inputSchema:{type:'object',properties:{preview:{type:'string',enum:['short','full'],default:'short',description:'preview'},limit:{type:'integer',minimum:1,maximum:100,description:'limit'}}}};
const upstream = http.createServer(async (req,res) => {
  const chunks=[];for await (const c of req) chunks.push(c);
  const rpc=JSON.parse(Buffer.concat(chunks));seen.push(rpc);
  assert.equal(req.headers['mcp-protocol-version'],'2025-03-26');
  const result=rpc.method==='tools/list'?{tools:[tool,{name:'zvec_grep_callers',description:'unchanged',inputSchema:{}}]}:rpc.method==='initialize'?{instructions:'unchanged guidance'}:{content:[{type:'text',text:JSON.stringify(rpc.params.arguments)}]};
  res.writeHead(200,{'Content-Type':sse?'text/event-stream':'application/json','Mcp-Session-Id':'same-session'});
  const value=JSON.stringify({jsonrpc:'2.0',id:rpc.id,result});res.end(sse?'data: \n\ndata: '+value+'\n\n':value);
});
await new Promise(r=>upstream.listen(0,'127.0.0.1',r));
const proxy=await startProxy('http://127.0.0.1:'+upstream.address().port+'/mcp',process.env.LOG);
async function rpc(method,params={}) {
 const res=await fetch(proxy.url,{method:'POST',headers:{'Content-Type':'application/json','MCP-Protocol-Version':'2025-03-26'},body:JSON.stringify({jsonrpc:'2.0',id:seen.length+1,method,params})});
 assert.equal(res.headers.get('mcp-session-id'),'same-session');
 const body=await res.text();return JSON.parse(sse?body.split('\n').find(l=>l.startsWith('data:')&&l.slice(5).trim()).slice(5).trim():body).result;
}
try {
 assert.equal((await rpc('initialize')).instructions,'unchanged guidance');
 for (const mode of [false,true]) {
  sse=mode;const surface=await rpc('tools/list');
  assert.equal(surface.tools[0].inputSchema.properties.preview.const,'short');
  assert.equal(surface.tools[0].inputSchema.properties.limit.const,20);
  assert(!surface.tools[0].description.includes('preview: "full"'));
  assert.equal(surface.tools[1].description,'unchanged');
  await rpc('tools/call',{name:'zvec_grep_search',arguments:{root:'/app',query:'concept',preview:'full',limit:50}});
  assert.deepEqual(seen.at(-1).params.arguments,{root:'/app',query:'concept',preview:'short',limit:20});
 }
 sse=false;
 await rpc('tools/call',{name:'zvec_grep_search',arguments:{root:'/app',fts:'symbol'}});
 assert.deepEqual(seen.at(-1).params.arguments,{root:'/app',fts:'symbol',preview:'short',limit:20});
 await rpc('tools/call',{name:'zvec_grep_callers',arguments:{root:'/app',symbol:'Class::method',limit:50}});
 assert.deepEqual(seen.at(-1).params.arguments,{root:'/app',symbol:'Class::method',limit:50});
} finally {
 await proxy.close();await new Promise(r=>{upstream.close(r);upstream.closeAllConnections();});
}
'''
        with tempfile.TemporaryDirectory() as temporary:
            log = Path(temporary) / 'execution.jsonl'
            environment = {**os.environ, 'PROXY': (ROOT / 'zg_bench/fixed_search_proxy.mjs').as_uri(), 'LOG': str(log)}
            result = subprocess.run(['node', '--input-type=module', '--eval', script], env=environment, capture_output=True, text=True, timeout=30)
            self.assertEqual(result.returncode, 0, result.stderr)
            audit = audit_fixed_search(log)
            self.assertEqual(audit['search_calls'], 3)
            self.assertEqual(audit['normalized_search_calls'], 3)
            self.assertEqual(len(audit['results']), 4)
            self.assertFalse(audit['transport_errors'])
            events = log.read_text().splitlines()
            log.write_text('\n'.join(events[:-1]))
            with self.assertRaisesRegex(ValueError, 'did not complete'):
                audit_fixed_search(log)

    def test_candidate_specific_surfaces_and_small_scope(self):
        frozen = json.loads((ROOT / 'fixtures/caller-callee-fixed-search-v1.json').read_text())
        self.assertEqual(experiment_tasks('qa20','guidance-fixed-search'), ['matplotlib-37','sympy-38','xarray-32'])
        for candidate, value in frozen['candidates'].items():
            for field, raw in [('groups', False), ('raw_groups', True)]:
                for group, digest in value[field].items():
                    validate_fixed_surface(candidate, group, {'sha256':digest}, raw=raw)
                    with self.assertRaisesRegex(ValueError, 'frozen fixed-search candidate'):
                        validate_fixed_surface(candidate, group, {'sha256':'wrong'}, raw=raw)


class FixedSearchSetupTests(unittest.IsolatedAsyncioTestCase):
    async def test_model_config_and_surface_probe_use_same_controlled_endpoint(self):
        with tempfile.TemporaryDirectory() as temporary:
            agent = ZvecOpenCode(logs_dir=Path(temporary), embedding_model='local/potion-code-16m-v2',
                mcp_target='opencode', mcp_toolset='agent', graph_index=True, guidance_variant='direct', fixed_search=True,
                opencode_config={'mcp':{'zvec_grep':{'type':'remote','url':'http://127.0.0.1:7999/mcp'}}})
            environment = SimpleNamespace(upload_file=AsyncMock())
            async def execute(*args, **kwargs):
                ready = {'url':'http://127.0.0.1:12345/mcp','policy':{'preview':'short','limit':20}}
                return subprocess.CompletedProcess([],0,stdout=json.dumps(ready) if 'cat /logs/agent/mcp-proxy.json' in kwargs['command'] else '',stderr='')
            metadata = {}
            with patch.object(agent,'exec_as_agent',new=AsyncMock(side_effect=execute)) as run:
                await agent._setup_mcp(environment,'/app',metadata,supports_ready_check=True)
            self.assertEqual(environment.upload_file.await_count,2)
            commands = [call.kwargs['command'] for call in run.call_args_list]
            self.assertTrue(any('7999/mcp' in command and 'tool-surface.raw.json' in command for command in commands))
            self.assertIn('12345/mcp',commands[-1])
            self.assertIn('12345/mcp',agent._build_register_config_command())
            self.assertEqual(metadata['fixed_search'],{'preview':'short','limit':20})


if __name__ == '__main__':
    unittest.main()
