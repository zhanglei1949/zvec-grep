# G4 之前的验证记录归档

原 knowledge/reports 下 24 个早期目录已完整归档，G4 保留原位。共 1,050 个文件、129,831,971 字节，包含忽略日志、成功与失败记录、来源清单及退役组件快照。历史结果仅适用于原记录的源码、依赖和环境，不代表当前版本验证。

## 归档位置

本机备份目录：

/Users/zhanglei/code/aone/knowledge-graph-design/backups/zg-pre-g4-reports-20261030-111142

目录内包含 reports.tar.gz、manifest.json 和恢复说明。压缩包 21,087,993 字节，SHA-256：

3fe7c8a1a8c1ca243a315bdadb9706bc19c8f05d68bbf55c2ae68859ddfa2b58

本仓保留[完整清单](manifest.json)，压缩包未放入 Git，也未上传或发布。换机恢复需先复制上述备份目录；不能仅凭本索引取得原始文件。

## 恢复原始记录

在仓库根运行以下命令；仅恢复到新的临时目录，不覆盖工作树。归档制作时已实际完成一次同样的逐文件恢复校验。

~~~python
from pathlib import Path
import hashlib, json, tarfile, tempfile

manifest = json.loads(Path("knowledge/archive/pre-g4-validation/manifest.json").read_text())
archive = Path(manifest["destination"]) / manifest["archive_file"]

def digest(path):
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()

assert digest(archive) == manifest["archive_sha256"]
restored = Path(tempfile.mkdtemp(prefix="zg-pre-g4-"))
with tarfile.open(archive) as tar:
    tar.extractall(restored, filter="data")
assert {str(p.relative_to(restored)) for p in restored.rglob("*") if p.is_file()} == {e["path"] for e in manifest["files"]}
for entry in manifest["files"]:
    path = restored / entry["path"]
    assert path.stat().st_size == entry["bytes"]
    assert digest(path) == entry["sha256"]
print(restored)
~~~

Python 3.12+。文档证据链接指向下列目录条目，悬停链接可查看具体包内路径；完整文件列表见 manifest.json。

## 仍在使用的输入

G2.4 来源清单另以原始字节保留在 [构建分析夹具](../../../rust/benchmarks/graph-build/fixtures/g2.4-corpus-manifest.json)，由 analyze.py 读取。后续本地运行默认输出到 knowledge/.artifacts/graph-build，不再生成旧报告目录。

## 目录清单

<a id="ann-readonly-fix"></a>

### ann-readonly-fix

包内目录：knowledge/reports/ann-readonly-fix/；16 个文件，1,754,900 字节。

<a id="code-coverage"></a>

### code-coverage

包内目录：knowledge/reports/code-coverage/；73 个文件，2,002,618 字节。

<a id="code-readiness"></a>

### code-readiness

包内目录：knowledge/reports/code-readiness/；81 个文件，68,577,359 字节。

<a id="g2-g3-handoff"></a>

### g2-g3-handoff

包内目录：knowledge/reports/g2-g3-handoff/；23 个文件，2,641,264 字节。

<a id="g2-0"></a>

### g2.0

包内目录：knowledge/reports/g2.0/；5 个文件，22,603 字节。

<a id="g2-1"></a>

### g2.1

包内目录：knowledge/reports/g2.1/；13 个文件，225,670 字节。

<a id="g2-1-followup"></a>

### g2.1-followup

包内目录：knowledge/reports/g2.1-followup/；11 个文件，119,089 字节。

<a id="g2-2-batch-recovery"></a>

### g2.2-batch-recovery

包内目录：knowledge/reports/g2.2-batch-recovery/；19 个文件，5,975,387 字节。

<a id="g2-2-build-commit"></a>

### g2.2-build-commit

包内目录：knowledge/reports/g2.2-build-commit/；6 个文件，529,351 字节。

<a id="g2-2-host-publication"></a>

### g2.2-host-publication

包内目录：knowledge/reports/g2.2-host-publication/；27 个文件，868,749 字节。

<a id="g2-2-native-control"></a>

### g2.2-native-control

包内目录：knowledge/reports/g2.2-native-control/；25 个文件，5,025,801 字节。

<a id="g2-3-recovery-leases"></a>

### g2.3-recovery-leases

包内目录：knowledge/reports/g2.3-recovery-leases/；42 个文件，10,207,466 字节。

<a id="g2-3-retention"></a>

### g2.3-retention

包内目录：knowledge/reports/g2.3-retention/；38 个文件，5,121,211 字节。

<a id="g2-4-resources"></a>

### g2.4-resources

包内目录：knowledge/reports/g2.4-resources/；138 个文件，1,652,214 字节。

<a id="g3-0-contracts"></a>

### g3.0-contracts

包内目录：knowledge/reports/g3.0-contracts/；12 个文件，22,858 字节。

<a id="g3-1"></a>

### g3.1

包内目录：knowledge/reports/g3.1/；15 个文件，384,709 字节。

<a id="g3-1-review"></a>

### g3.1-review

包内目录：knowledge/reports/g3.1-review/；16 个文件，53,086 字节。

<a id="g3-2"></a>

### g3.2

包内目录：knowledge/reports/g3.2/；52 个文件，851,150 字节。

<a id="g3-3"></a>

### g3.3

包内目录：knowledge/reports/g3.3/；27 个文件，344,397 字节。

<a id="graph-build-optimization"></a>

### graph-build-optimization

包内目录：knowledge/reports/graph-build-optimization/；138 个文件，17,199,852 字节。

<a id="impl-calls"></a>

### impl-calls

包内目录：knowledge/reports/impl-calls/；59 个文件，438,393 字节。

<a id="markdown-unification"></a>

### markdown-unification

包内目录：knowledge/reports/markdown-unification/；26 个文件，52,464 字节。

<a id="module-consolidation"></a>

### module-consolidation

包内目录：knowledge/reports/module-consolidation/；55 个文件，5,051,424 字节。

<a id="query-qualification"></a>

### query-qualification

包内目录：knowledge/reports/query-qualification/；133 个文件，709,956 字节。
