"""Build the official-release frozen index without running an answering model."""
from .release_opencode import ReleaseOpenCode
from ..release_packages import write_json


class ReleaseIndexOnly(ReleaseOpenCode):
    async def run(self, instruction, environment, context):
        write_json(self.logs_dir / "index-only.json", {"model_requests": 0, "answering_started": False})
