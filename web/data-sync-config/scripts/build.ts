export {};

const result = await Bun.build({
  entrypoints: ["src/main.ts"],
  outdir: "dist",
  target: "browser",
  minify: true,
  naming: "app.js",
});

if (!result.success) {
  for (const log of result.logs) {
    console.error(log);
  }
  process.exit(1);
}

const outputPath = "dist/app.js";
const output = await Bun.file(outputPath).text();
await Bun.write(outputPath, output.replaceAll("\t", "  "));
