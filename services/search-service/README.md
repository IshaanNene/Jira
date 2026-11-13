# search-service

Full-text issue search on OpenSearch. It owns no data of record: the index can be dropped and rebuilt from
issue-service at any time.

**Indexing.** Consumes the issue and project topics. For any `issue.*` or `comment.*` event it fetches the issue's
current document with `issue.v1.Issues/GetDocuments` (key, summary, plain-text description and comments, status,
priority, people, labels, dates) and writes it with external versioning (`external_gte` on the issue version), so
a late or replayed event cannot overwrite newer content. `issue.deleted`, or an issue the source reports missing,
removes the document; `project.deleted` removes the project's documents. Control and private-use characters are
stripped and long text is truncated to 32 KB per field.

**Isolation.** One shared index (`issues-v1`); each document carries `tenant_id`, is stored as
`{tenant}:{issue}` and routed by tenant. Every query and delete is built inside the service with an unconditional
`tenant_id` filter from the verified token or event envelope; nothing from the client reaches OpenSearch except
the search words, enumerated filters and a validated cursor. Results are further limited to the projects the
caller can browse; asking for a hidden project returns nothing.

**API.** `GET /api/v1/search/issues?q&project_id&status_category&assignee=me|<id>&limit&cursor` (contract at
`/openapi.json`). Matches key (exact, boosted), summary (prefix and typo-tolerant), description, comments and
labels; an empty `q` lists recently updated issues. Matched text comes back as `{text, matched}` runs, never HTML.
Totals are counted up to 1000.

**Reindex.** `search-service reindex <tenant-id>` pages through `ListIssueIds`, rewrites every document, then
deletes the tenant's documents that were not rewritten (issues that no longer exist). Safe while the consumer runs.

| Variable | Required | Default |
|---|---|---|
| `OPENSEARCH_URL`, `REDIS_URL`, `TOKEN_ISSUER`, `JWKS_URL`, `INTERNAL_SERVICE_SECRET` | yes | |
| `KAFKA_BROKERS` | no | unset: the index does not follow changes |
| `SEARCH_INDEX`, `SEARCH_INDEX_SHARDS`, `SEARCH_INDEX_REPLICAS` | no | `issues-v1`, 1, 0 |
| `PERMISSION_SERVICE_GRPC_URL`, `ISSUE_SERVICE_GRPC_URL` | no | local ports 9108 / 9105 |
| `ALLOWED_ORIGINS`, `HTTP_ADDR` | no | –, `127.0.0.1:8111` |

**Tests:** `cargo test -p search-service` (needs OpenSearch from `make infra`; each test uses its own index).

**Not built yet.** The query language (JQL-like, Phase 6); custom fields, components and versions in the index;
reindexing every tenant in one command; index aliases for zero-downtime mapping changes; per-tenant or dedicated
indexes for very large tenants; search over projects, people or pages.
