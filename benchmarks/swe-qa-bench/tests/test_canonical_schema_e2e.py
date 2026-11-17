import copy
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from zg_bench.canonical_schema_e2e import (
    BASELINE, bounded_answer, check_surfaces, cohort_gate, invalid_streak,
    source_valid_results,
)


def canonical_surface():
    legacy = json.loads(BASELINE.read_text())
    canonical = copy.deepcopy(legacy)
    for tool in canonical["tools"]:
        schema = tool["inputSchema"]
        schema.pop("$defs")
        schema["properties"]["globs"] = {"type": "array", "items": {"type": "object"}}
        schema["properties"]["insensitiveGlobs"] = {"type": "array", "items": {"type": "string"}}
    return legacy, canonical


def invalid(identity, error="same parse error", tool="invalid"):
    return {"type": "tool_use", "part": {"callID": identity, "tool": tool,
            "state": {"status": "completed", "input": {"error": error}}}}


class CanonicalSchemaAcceptanceTests(unittest.TestCase):
    def test_only_filter_schema_changes_and_guidance_stays_identical(self):
        old, new = canonical_surface()
        check_surfaces(old, new, old)
        for mutation in [
            lambda x: x.update(instructions=x["instructions"] + " changed"),
            lambda x: x["tools"][0].update(description="changed"),
            lambda x: x["tools"][0]["inputSchema"]["properties"]["limit"].update(maximum=99),
            lambda x: x["tools"][0]["inputSchema"]["properties"]["globs"].update(type="string"),
            lambda x: x["tools"][0]["inputSchema"].update(required=["root", "symbol", "globs"]),
        ]:
            changed = copy.deepcopy(new)
            mutation(changed)
            with self.assertRaises(ValueError):
                check_surfaces(old, changed, old)

    def test_invalid_stop_counts_distinct_consecutive_matching_failures(self):
        self.assertEqual(invalid_streak([invalid("1"), invalid("1"), invalid("2")]), 2)
        self.assertEqual(invalid_streak([invalid("1"), invalid("2", tool="read"), invalid("3")]), 1)
        self.assertEqual(invalid_streak([invalid("1"), invalid("2", "different")]), 1)
        self.assertEqual(invalid_streak([invalid(str(i)) for i in range(3)]), 3)

    @unittest.skipIf(os.name == "nt", "Linux diagnostic uses POSIX process groups")
    def test_repeated_invalid_child_stops_and_retains_raw_events(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            program = "import json,time; events=" + repr([invalid(str(i)) for i in range(3)]) + "; [print(json.dumps(e),flush=True) for e in events]; time.sleep(30)"
            with (root / "events.jsonl").open("w") as stdout, (root / "stderr").open("w") as stderr:
                result = bounded_answer([sys.executable, "-c", program], cwd=root, env=os.environ,
                                        stdout=stdout, stderr=stderr, timeout=5)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(result.stop_reason, "three_consecutive_identical_invalid_actions")
            self.assertEqual(len((root / "events.jsonl").read_text().splitlines()), 3)

    def test_source_gate_requires_real_success_and_valid_positions(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name) / "app"
            root.mkdir()
            source = "class Coordinates:\n    def update(self):\n        operation()\n"
            (root / "coordinates.py").write_text(source)
            (root / "ops.py").write_text("def operation():\n    pass\n")
            output = Path(name) / "evidence"
            directory = output / "trial-1"
            directory.mkdir(parents=True)
            (output / "source-definitions.json").write_text(json.dumps([
                {"symbol": "Coordinates::update", "file": "coordinates.py", "line": 2, "end_line": 3}]))
            arguments = {"root": str(root), "symbol": "Coordinates.update", "globs": "coordinates.py", "limit": 20}
            call = {"type": "call", "sequence": 1, "name": "zvec_grep_callees", "requested_arguments": arguments, "effective_arguments": arguments}
            # An explicit import independently establishes the cross-file binding.
            source = "from ops import operation\nclass Coordinates:\n    def update(self):\n        operation()\n"
            (root / "coordinates.py").write_text(source)
            (output / "source-definitions.json").write_text(json.dumps([
                {"symbol": "Coordinates::update", "file": "coordinates.py", "line": 3, "end_line": 4}]))
            text = "Coordinates::update  coordinates.py:3\n  Callees:\n    operation  ops.py:1\n      call at coordinates.py:4:9\n"
            response = {"type": "result", "sequence": 1, "http_status": 200, "result": {"content": [{"type": "text", "text": text}]}}
            path = directory / "mcp-execution.jsonl"
            path.write_text(json.dumps(call) + "\n" + json.dumps(response) + "\n")
            checks = source_valid_results(directory, root)
            self.assertTrue(cohort_gate({"complete": True}, checks))
            self.assertFalse(cohort_gate({"complete": False}, checks))
            self.assertFalse(cohort_gate({"complete": True}, []))
            for broken in [text.replace(":4:9", ":4:1"), text.replace("ops.py:1", "ops.py:2"), text.replace("coordinates.py:3", "coordinates.py:1"), "No matches.\n"]:
                response["result"]["content"][0]["text"] = broken
                path.write_text(json.dumps(call) + "\n" + json.dumps(response) + "\n")
                self.assertFalse(cohort_gate({"complete": True}, source_valid_results(directory, root)))
            # Both locations can exist while a builtin is bound to the wrong method.
            (root / "coordinates.py").write_text(source + "        len([])\n        self._write()\n    def _write(self):\n        pass\n")
            (root / "ops.py").write_text("def operation():\n    pass\nclass StringAccessor:\n    def len(self):\n        pass\n")
            (output / "source-definitions.json").write_text(json.dumps([
                {"symbol": "Coordinates::update", "file": "coordinates.py", "line": 3, "end_line": 6}]))
            wrong = "    StringAccessor::len  ops.py:4\n      call at coordinates.py:5:9\n"
            method = "    Coordinates::_write  coordinates.py:7\n      call at coordinates.py:6:9\n"
            response["result"]["content"][0]["text"] = text + wrong + method
            path.write_text(json.dumps(call) + "\n" + json.dumps(response) + "\n")
            check = source_valid_results(directory, root)[0]
            self.assertTrue(check["source_valid_nonempty"])
            self.assertEqual(len(check["verified_relationships"]), 2)
            self.assertEqual(len(check["unverified_relationships"]), 1)
            response["result"]["content"][0]["text"] = text.split("    operation")[0] + wrong
            path.write_text(json.dumps(call) + "\n" + json.dumps(response) + "\n")
            self.assertFalse(cohort_gate({"complete": True}, source_valid_results(directory, root)))

    def test_actual_request_guard_rejects_legacy_schema_without_rewriting_valid_body(self):
        recorder = Path(__file__).parents[1] / "zg_bench/model_request_recorder.mjs"
        script = r'''
import assert from 'node:assert/strict';
import http from 'node:http';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {createInterface} from 'node:readline';
import {writeFileSync,readFileSync} from 'node:fs';
const seen=[];
const upstream=http.createServer(async(req,res)=>{
 const chunks=[];for await(const c of req)chunks.push(c);seen.push(Buffer.concat(chunks).toString());
 res.writeHead(200,{'Content-Type':'text/event-stream'});res.end('data: [DONE]\n\n');
});
await new Promise(r=>upstream.listen(0,'127.0.0.1',r));
const name='zvec_grep_zvec_grep_callees';
writeFileSync(process.env.CONTRACT,JSON.stringify({guidance:'same agents',instructions:'same instructions',tool_names:[name],schema_profile:'canonical-relationships'}));
const child=spawn('node',[process.env.RECORDER,'http://127.0.0.1:'+upstream.address().port+'/v1',process.env.LOG,process.env.CLOSED,process.env.CONTRACT],{stdio:['ignore','pipe','pipe']});
const lines=createInterface({input:child.stdout});const control=JSON.parse((await once(lines,'line'))[0]);
const body={model:'glm-5.2',messages:[{role:'system',content:'same agents\n<mcp_instructions><server name="zvec_grep">same instructions</server></mcp_instructions>'},{role:'user',content:'question'}],tools:[{type:'function',function:{name,parameters:{type:'object',properties:{globs:{anyOf:[{type:'string'}]},insensitiveGlobs:{type:'array',items:{type:'string'}}}}}}],tool_choice:'auto',stream:true};
async function send(){const r=await fetch(control.baseURL+'/chat/completions',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)});await r.text();return r.status;}
assert.equal(await send(),400);assert.equal(seen.length,0);
body.tools[0].function.parameters.properties.globs={type:'array',items:{type:'object'}};
assert.equal(await send(),200);assert.deepEqual(seen,[JSON.stringify(body)]);
const exited=once(child,'exit');child.kill('SIGTERM');assert.equal((await exited)[0],0);
const records=readFileSync(process.env.LOG,'utf8').trim().split('\n').map(JSON.parse);
assert.equal(records.filter(e=>e.type==='guidance_rejected').length,1);
assert.equal(records.filter(e=>e.type==='guidance_verified'&&e.first_task_request).length,1);
await new Promise(r=>upstream.close(r));
'''
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            env = {**os.environ, "RECORDER": str(recorder), "CONTRACT": str(root / "contract.json"),
                   "LOG": str(root / "requests.jsonl"), "CLOSED": str(root / "closed")}
            result = subprocess.run(["node", "--input-type=module", "--eval", script], env=env,
                                    capture_output=True, text=True, timeout=20)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
