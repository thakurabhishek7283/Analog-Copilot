// contract/schema/contract.schema.json -> apps/web/src/gen/contract.ts
// Every $defs entry becomes an exported type. Output is committed; CI regenerates and diffs.
import { compile } from "json-schema-to-typescript";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "../..");
const schema = JSON.parse(readFileSync(join(root, "contract/schema/contract.schema.json"), "utf8"));
const ts = await compile(schema, "Contract", {
  bannerComment:
    "/* Generated from contract/schema/contract.schema.json by tools/codegen. Do not edit. */",
  unreachableDefinitions: true,
  additionalProperties: false,
  strictIndexSignatures: true,
  format: true,
  style: { printWidth: 110, singleQuote: false },
});
const out = join(root, "apps/web/src/gen/contract.ts");
mkdirSync(dirname(out), { recursive: true });
writeFileSync(out, ts.replace(/\r\n/g, "\n"));
console.log("wrote apps/web/src/gen/contract.ts");
