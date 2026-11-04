import { main } from "./engines/zg/report.mjs";
import { runCli } from "./core/cli.mjs";
import { G4_PROFILES } from "./core/g4.mjs";

runCli(import.meta.url, async () => {
  const args = process.argv.slice(2);
  const index = args.indexOf("--protocol");
  if (index < 0) return main(args);
  const profile = args[index + 1];
  if (!G4_PROFILES.includes(profile))
    throw new Error(`unknown protocol: ${profile}`);
  args.splice(index, 2);
  return (await import("./engines/zg/g4-report.mjs")).main(args, profile);
});
