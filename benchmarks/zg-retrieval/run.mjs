import { main } from "./engines/zg/run.mjs";
import { runCli } from "./core/cli.mjs";

runCli(import.meta.url, async () => {
  const args = process.argv.slice(2);
  const index = args.indexOf("--protocol");
  if (index < 0) return main(args);
  const profile = args[index + 1];
  if (profile !== "g4-code-v7") throw new Error(`unknown protocol: ${profile}`);
  args.splice(index, 2);
  return (await import("./engines/zg/g4-run.mjs")).main(args);
});
