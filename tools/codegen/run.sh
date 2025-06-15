#!/usr/bin/env bash
# Regenerate everything derived from circuit-core's Rust types (LLD §2):
#   contract/schema/*.json  ->  apps/web/src/gen/contract.ts  +  apps/api/tutor_api/models/contract.py
# usage: tools/codegen/run.sh [--check]    (--check: fail if any generated file changed; for CI)
set -euo pipefail
cd "$(dirname "$0")/../.."

cargo run -q -p circuit-core --example export_schema >/dev/null

(cd tools/codegen && npm ci --silent && node gen_ts.mjs >/dev/null \
  && npx tsc --noEmit --strict --target es2022 ../../apps/web/src/gen/contract.ts)

PYTHONWARNINGS=ignore uvx -q --from datamodel-code-generator==0.83.0 --with black==26.5.1 --with isort==8.0.1 \
  datamodel-codegen \
  --input contract/schema/contract.schema.json --input-file-type jsonschema \
  --output apps/api/tutor_api/models/contract.py \
  --output-model-type pydantic_v2.BaseModel --target-python-version 3.12 \
  --formatters black isort --disable-timestamp --use-annotated --field-constraints --use-double-quotes \
  --use-standard-collections --use-union-operator --use-subclass-enum --use-schema-description \
  --custom-file-header "# Generated from contract/schema/contract.schema.json by tools/codegen. Do not edit."
# datamodel-codegen writes the platform's line endings; the repo stores LF everywhere.
sed -i 's/$//' apps/api/tutor_api/models/contract.py

if [ "${1:-}" = "--check" ]; then
  changed=$(git status --porcelain -- contract apps/web/src/gen apps/api/tutor_api/models)
  if [ -n "$changed" ]; then
    echo "Generated code is stale or uncommitted:"; echo "$changed"; exit 1
  fi
fi
echo "codegen ok"
