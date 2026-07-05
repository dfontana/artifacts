---
name: artifacts-api-spec
description: Fetch the open-api specification for a specific action, endpoint, or request to the artifacts API. Use when investigating or implementing any artifacts API related interactions, rather than guessing at artifacts api.
user-invocable: false
---

# Artifacts Api Spec

## Instructions: API Specification
Follow this section for any HTTP interaction tasks.
1. Run `bash scripts/cache-spec.sh`
2. Search [spec.json](./spec.json) for the endpoint or model you're seeking using `rg`.
3. Extract the data model or endpoint specifics using `jq`
4. Optionally, fetch additional context on the following subjects only if directly related to your task:
  - Rate limit rules: https://docs.artifactsmmo.com/api_guide/rate_limits/
  - Specific 4XX-5XX response code meanings: https://docs.artifactsmmo.com/api_guide/response_codes/

## Instructions: Game Concepts
Follow this section to learn about any specific game semantic, something not codified into the api specification itself. Identify your concept category from this table and web-fetch the related page:
| Category | Link |
|:-|:-|
|Seasons|https://docs.artifactsmmo.com/concepts/seasons/|
|Achievements|https://docs.artifactsmmo.com/concepts/achievements/|
|Actions & Cooldowns|https://docs.artifactsmmo.com/concepts/actions/|
|Maps & Movement|https://docs.artifactsmmo.com/concepts/maps_and_movement/|
|Combat & Stats|https://docs.artifactsmmo.com/concepts/stats_and_fights/|
|Equipment|https://docs.artifactsmmo.com/concepts/equipment/|
|Skills|https://docs.artifactsmmo.com/concepts/skills/|
|Recycling|https://docs.artifactsmmo.com/concepts/recycling/|
|Resting & Using items|https://docs.artifactsmmo.com/concepts/resting_and_using_items/|
|Give Items & Gold|https://docs.artifactsmmo.com/concepts/give/|
|Pending Items|https://docs.artifactsmmo.com/concepts/pending_items/|
|Inventory & Bank|https://docs.artifactsmmo.com/concepts/inventory_and_bank/|
|Grand Exchange|https://docs.artifactsmmo.com/concepts/grand_exchange/|
|NPCs|https://docs.artifactsmmo.com/concepts/npcs/|
|Tasks|https://docs.artifactsmmo.com/concepts/tasks/|
|Events|https://docs.artifactsmmo.com/concepts/events/|
|Raids|https://docs.artifactsmmo.com/concepts/raids/|
|Gems Shop|https://docs.artifactsmmo.com/concepts/gems-shop/|
```
