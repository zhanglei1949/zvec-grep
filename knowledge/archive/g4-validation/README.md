# G4 验证证据归档

[最终报告](../../G4_VALIDATION_REPORT.md)保留原有结果和失败边界。本索引不将局部收益解释为整体验证通过。

## 原始证据

备份目录：/Users/zhanglei/code/aone/knowledge-graph-design/backups/zg-g4-validation-20261030-112020

evidence.tar.gz 包含 1,519 个原始证据文件，以及 candidate/ 下的冻结候选包。正式调用、准备失败、初版评分、空 stdout/stderr、来源快照与日志均保留原始路径和字节。

压缩包大小：43,810,081 字节。SHA-256：

ca262abe05849c778c3947ec7bc354cb70a848739fed6284d1d8e370cf0ff79c

[manifest.json](manifest.json)列出逐文件路径、大小和摘要；[标注问题](annotation-review.json)、[原验证汇总](verification.json)为原件副本，未修改严格评分。

安装目录 consumer、npm-cache 和 runtime-home 的本机配置／授权状态不入证据包。它们可由候选包重新准备，不作为评分输入。排除路径列于清单；候选包已按原记录摘要核对。

压缩包保存在本机设计资料目录，未放入 Git，也未上传或发布。换机需先复制备份目录。

## 恢复与校验

在仓库根使用 Python 3.12+ 执行以下代码，只解压到新的临时目录：

~~~python
from pathlib import Path
import hashlib, json, tarfile, tempfile

manifest = json.loads(Path("knowledge/archive/g4-validation/manifest.json").read_text())
archive = Path(manifest["destination"]) / manifest["archive_file"]

def digest(path):
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()

assert digest(archive) == manifest["archive_sha256"]
restored = Path(tempfile.mkdtemp(prefix="zg-g4-"))
with tarfile.open(archive) as tar:
    tar.extractall(restored, filter="data")
expected = {e["path"] for e in manifest["files"]} | {manifest["candidate_archive_path"]}
assert {str(p.relative_to(restored)) for p in restored.rglob("*") if p.is_file()} == expected
for entry in manifest["files"]:
    path = restored / entry["path"]
    assert path.stat().st_size == entry["bytes"]
    assert digest(path) == entry["sha256"]
assert digest(restored / manifest["candidate_archive_path"]) == manifest["candidate"]["sha256"]
print(restored)
~~~

## 离线重算

以打印的恢复目录替换 RESTORED_DIR，在仓库根执行：

~~~sh
node benchmarks/zg-retrieval/report.mjs --protocol g4-code-v7 \
  RESTORED_DIR/knowledge/reports/g4/run-5
~~~

已在恢复副本上实际执行，报告摘要为 fc2b029255b00663b6ef9c1937a9a06ef5351772e20060d49f6d07207d4e4b83，与原报告一致。退出码 1 保留原始预算错误导致的完整性失败，不能改写成全部通过。重算只读取归档证据，不运行新的检索，也不需要被排除的安装缓存和授权状态。

复跑新实验需按 benchmarks/zg-retrieval/README.md 准备新语料、模型和索引；这是不同于离线重算的工作。输出目录可使用 knowledge/.artifacts/，不要覆盖冻结记录。
