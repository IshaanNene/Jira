# search-service

Full-text issue search on OpenSearch. The index is not data of record: it can be dropped and rebuilt from
issue-service at any time. Saved MQL filters are, and live in PostgreSQL (`search_db`, row-level security by tenant):
`/api/v1/search/filters` lists, creates, reads, updates and deletes them, and every write parses the query.

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

**MQL.** `GET /api/v1/search/issues/query?mql&limit&cursor` runs a query in the issue query language, e.g.
`project = PAY AND assignee = currentUser() AND priority >= High ORDER BY updated DESC`
(`src/mql.rs`). Fields: `project`, `key`, `type`, `status`, `statusCategory`, `priority`, `assignee`, `reporter`,
`labels`, `text` / `summary` / `description` / `comment` (`~`, `!~`, trailing `*` for prefixes), `created`,
`updated`, `resolved` (dates, `-7d`-style relative times, `now()`, `startOf{Day,Week,Month,Year}()`, UTC).
Operators: `= != < <= > >= ~ !~ IN (…) NOT IN (…) IS [NOT] EMPTY`, with `AND`, `OR`, `NOT` and parentheses;
`ORDER BY created|updated|resolved|priority|key [ASC|DESC]` (keys sort by number, priorities by rank).

The query is lexed, parsed and compiled here into an OpenSearch query; typed values only become term values,
analysed match text or date math assembled from validated parts, so nothing a person types is interpreted by
OpenSearch as syntax. The compiled query always sits inside the tenant and visible-project filters. Limits:
2000 characters, 100 conditions, nesting depth 16, 100 values per list, 3 sort fields. A cursor must match the
query's own sort shape. An invalid query is a 422 whose `errors[0].span` is the `[start, end)` character range at
fault, with a plain-language message.

People are named by `currentUser()`, a user id, or an exact email address or display name, resolved through
`user.v1.Membership/FindMembers`; a name shared by several people is refused with a request to use the email.
Guests, who cannot see the member directory, can only use `currentUser()` and ids. `!=` and `NOT IN` include
issues where the field is empty.

**Reindex.** `search-service reindex <tenant-id>` pages through `ListIssueIds`, rewrites every document, then
deletes the tenant's documents that were not rewritten (issues that no longer exist). Safe while the consumer runs.

| Variable | Required | Default |
|---|---|---|
| `OPENSEARCH_URL`, `REDIS_URL`, `TOKEN_ISSUER`, `JWKS_URL`, `INTERNAL_SERVICE_SECRET` | yes | |
| `KAFKA_BROKERS` | no | unset: the index does not follow changes |
| `SEARCH_INDEX`, `SEARCH_INDEX_SHARDS`, `SEARCH_INDEX_REPLICAS` | no | `issues-v1`, 1, 0 |
| `PERMISSION_SERVICE_GRPC_URL`, `ISSUE_SERVICE_GRPC_URL`, `USER_SERVICE_GRPC_URL` | no | local ports 9108 / 9105 / 9103 |
| `ALLOWED_ORIGINS`, `HTTP_ADDR` | no | –, `127.0.0.1:8111` |

**Tests:** `cargo test -p search-service` (needs OpenSearch from `make infra`; each test uses its own index).

**Mapping changes.** Fields added after the index was created (`priority_rank`, `key_number`) are put on the
existing mapping at start-up; documents indexed earlier lack them until rewritten, so run `reindex` for each
tenant after upgrading.

**Not built yet.** Saved filters and subscriptions; custom fields, components, versions, sprints and parents in
the index (and so in MQL); functions beyond `currentUser()` and date functions (`membersOf()`, `openSprints()`);
history operators (`WAS`, `CHANGED`); time zones other than UTC;
reindexing every tenant in one command; index aliases for zero-downtime mapping changes; per-tenant or dedicated
indexes for very large tenants; search over projects, people or pages.
