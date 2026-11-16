# workflow-service

**Responsibility.** Workflow definitions (statuses, transitions and their rules), versioned publishing, the
assignment of workflows to projects and issue types, and the engine that decides whether a status change is
allowed.

**The model is declarative.** A transition carries conditions (who may use it), validators (what must be true of
the issue) and post-functions (what happens afterwards), each chosen from a closed set in `engine.rs`. There is
no expression language; an unknown rule type or an unexpected property fails to deserialize and is rejected.
Adding a capability means adding a variant here and its effect in issue-service — never executing user input.

| Kind | Available |
|---|---|
| Conditions | project role at least…, is assignee, is reporter |
| Validators | field required (built-in fields or `custom:<field id>`), all child issues done |
| Post-functions | assign to current user / reporter, clear assignee, set resolution, clear resolution |

**Versioning.** Edits go to a draft that never affects issues. Publishing validates the draft (unique keys, valid
references, every status reachable from the initial one) and writes a new immutable row in `workflow_versions`.
`GET .../versions/{n}` returns one as it was; `POST .../versions/{n}/restore` (admins, `If-Match`) makes it the
draft, replacing unpublished changes, so going back is an ordinary, checked publish.
Draft saves and publishing use `If-Match`.

**Resolution order** for an issue: project + issue type → project → workspace default + issue type → workspace
default → built-in default (To Do, In Progress, In Review, Done; free movement).

**API.** REST under `/api/v1/workflows`, `/api/v1/workflow-assignments`, `/api/v1/workflow-statuses` — contract at
`/openapi.json`. Internal gRPC ([`workflow.proto`](../../proto/platform/workflow/v1/workflow.proto)):
`GetWorkflow`, `ListTransitions`, `EvaluateTransition`. issue-service reports facts about the actor and issue;
the rules never leave this service.

**Permissions.** Organization admins create, edit and publish workflows and set the workspace default. Project
administrators choose workflows for their own project. Everyone in the organization can view.

**Database.** Owns `workflow_db`: `workflows`, `workflow_versions`, `workflow_assignments` (forced RLS), `outbox`,
`inbox`. Emits `workflow.published`, `workflow.updated`.

**Not built yet.** Transition screens (prompting for fields during a transition), status mapping when a published
change removes a status that issues are in (such issues can only move back to the initial status), restoring an
old version, per-transition automation beyond the post-functions above.

| Variable | Required | Default |
|---|---|---|
| `DATABASE_URL`, `DATABASE_MIGRATION_URL` | yes / for `migrate` | |
| `REDIS_URL`, `TOKEN_ISSUER`, `JWKS_URL`, `INTERNAL_SERVICE_SECRET` | yes | |
| `TENANT_SERVICE_GRPC_URL`, `PERMISSION_SERVICE_GRPC_URL` | no | local ports 9102 / 9108 |
| `ALLOWED_ORIGINS`, `HTTP_ADDR`, `GRPC_ADDR` | no | –, `127.0.0.1:8106`, `:9106` |

**Run / test / observe / image.** As for tenant-service, with `workflow-service`.
