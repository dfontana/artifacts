---
name: live-integration-test
description: Learn how to run a live request against the artifacts API using a test character and token/secret. Use when wanting to test something end to end, against the actual game servers.
---

# Live Integration Test

## Instructions
1. `source .envrc` from the root of the repository (do not read the file). This exports `ARTIFACTS_SECRET`.
2. Utilize character named `nillinbot`
3. Construct any `curl` commands or run any code necessary to exercise your test case

## Examples

Running this command will make the character rest in game, and can be used to inspect the API directly:
```
source .envrc && curl --request POST \
  --url https://api.artifactsmmo.com/my/nillinbot/action/rest \
  --header 'Accept: application/json' \
  --header "Authorization: Bearer $ARTIFACTS_SECRET" \
  --header 'Content-Type: application/json'
```
