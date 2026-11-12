import { zodResolver } from "@hookform/resolvers/zod";
import { Box, Rocket, Trash2, Users } from "lucide-react";
import { useEffect, useState, type FormEvent } from "react";
import { Controller, useForm } from "react-hook-form";
import { Navigate, Route, Routes, useNavigate, useParams } from "react-router-dom";
import { z } from "zod";

import { Badge, Button, Card, CardHeader, EmptyState, ErrorState, Field, Input, Page, RouteTabs, Select, Skeleton, Textarea, toast } from "@/components/ui";
import { applyServerErrors, FormError } from "@/modules/auth";
import { can, UserChip, useUserDirectory } from "@/modules/organizations";
import { ProjectWorkflowSettings } from "@/modules/workflows";
import { ApiError, errorMessage, type Project, type ProjectRole } from "@/services/api";
import { useSession } from "@/state/session";
import { formatDate } from "@/utils/format";

import {
  useComponentMutations,
  useComponents,
  useProject,
  useProjectAccess,
  useProjectAction,
  useProjectMemberMutations,
  useUpdateProject,
  useVersionMutations,
  useVersions,
} from "../api";
import { PROJECT_ROLE_DESCRIPTION, PROJECT_ROLE_LABEL, roleAtLeast } from "../roles";

const fail = (title: string) => (error: unknown) => toast.error(title, errorMessage(error));
const ROLE_OPTIONS = (["administrator", "member", "viewer"] as const).map((r) => ({ value: r, label: PROJECT_ROLE_LABEL[r], description: PROJECT_ROLE_DESCRIPTION[r] }));

export default function ProjectSettingsPage() {
  const { projectKey = "" } = useParams();
  const key = projectKey.toUpperCase();
  const project = useProject(key);
  const base = `/projects/${key}/settings`;

  if (project.isPending) return <Page title="Project settings"><Skeleton className="h-48" /></Page>;
  if (project.isError) return <Page title="Project settings"><ErrorState error={project.error} onRetry={() => void project.refetch()} /></Page>;
  if (!roleAtLeast(project.data.my_role, "administrator")) return <Navigate to={`/projects/${key}/issues`} replace />;

  return (
    <Page title="Project settings" description={project.data.name}>
      <RouteTabs tabs={[{ to: base, label: "Details", end: true }, { to: `${base}/access`, label: "Access" }, { to: `${base}/workflow`, label: "Workflow" }, { to: `${base}/components`, label: "Components" }, { to: `${base}/releases`, label: "Releases" }]} />
      <Routes>
        <Route index element={<Details project={project.data} />} />
        <Route path="access" element={<Access project={project.data} />} />
        <Route path="workflow" element={<ProjectWorkflowSettings projectId={project.data.id} />} />
        <Route path="components" element={<Components projectKey={key} />} />
        <Route path="releases" element={<Releases projectKey={key} />} />
      </Routes>
    </Page>
  );
}

const detailsSchema = z.object({
  name: z.string().trim().min(2, "Use at least 2 characters.").max(80, "Use 80 characters or fewer."),
  description: z.string().max(2000, "Use 2000 characters or fewer."),
  project_type: z.string(),
  lead_user_id: z.string(),
});
type DetailsValues = z.infer<typeof detailsSchema>;
const NO_LEAD = "none";

function Details({ project }: { project: Project }) {
  const navigate = useNavigate();
  const orgRole = useSession((s) => s.organization?.role);
  const { members } = useUserDirectory();
  const update = useUpdateProject(project.key);
  const actions = useProjectAction(project.key);
  const [formError, setFormError] = useState<string | null>(null);
  const [conflict, setConflict] = useState<Project | null>(null);
  const defaults = (p: Project): DetailsValues => ({ name: p.name, description: p.description, project_type: p.project_type, lead_user_id: p.lead_user_id ?? NO_LEAD });
  const { register, control, handleSubmit, reset, setError, formState: { errors, isSubmitting, isDirty } } = useForm<DetailsValues>({ resolver: zodResolver(detailsSchema), defaultValues: defaults(project) });

  // Adopt changes made elsewhere while this form is untouched.
  useEffect(() => {
    if (!isDirty) reset(defaults(project));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [project.version]);

  const save = async (values: DetailsValues, version: number) => {
    setFormError(null);
    setConflict(null);
    try {
      const saved = await update.mutateAsync({ patch: { ...values, lead_user_id: values.lead_user_id === NO_LEAD ? null : values.lead_user_id }, version });
      reset(defaults(saved));
      toast.success("Project updated");
    } catch (error) {
      if (error instanceof ApiError && error.isConflict && error.current) setConflict(error.current as Project);
      else setFormError(applyServerErrors(error, setError, ["name", "description", "project_type", "lead_user_id"]));
    }
  };
  const archived = project.status === "archived";

  return (
    <div className="space-y-5">
      <Card>
        <CardHeader title="Details" description={`Key ${project.key} · created ${formatDate(project.created_at)}`} />
        <form onSubmit={handleSubmit((v) => save(v, project.version))} noValidate className="space-y-4 p-4">
          <FormError message={formError} />
          {conflict && (
            <div role="alert" className="rounded border border-warning/30 bg-warning-soft px-3 py-2 text-sm text-warning">
              Someone else changed this project while you were editing.
              <div className="mt-2 flex gap-2">
                <Button size="sm" onClick={() => void handleSubmit((v) => save(v, conflict.version))()}>Keep my changes</Button>
                <Button size="sm" variant="ghost" onClick={() => { reset(defaults(conflict)); setConflict(null); }}>Use theirs</Button>
              </div>
            </div>
          )}
          <Field label="Name" error={errors.name?.message}><Input {...register("name")} /></Field>
          <Field label="Description" error={errors.description?.message}><Textarea rows={3} {...register("description")} /></Field>
          <div className="grid gap-4 sm:grid-cols-2">
            <Controller control={control} name="project_type" render={({ field }) => (
              <Field label="Type"><Select value={field.value} onValueChange={field.onChange} options={[{ value: "scrum", label: "Scrum" }, { value: "kanban", label: "Kanban" }, { value: "general", label: "General" }]} /></Field>
            )} />
            <Controller control={control} name="lead_user_id" render={({ field }) => (
              <Field label="Project lead"><Select value={field.value} onValueChange={field.onChange} options={[{ value: NO_LEAD, label: "No lead" }, ...members.filter((m) => m.status === "active").map((m) => ({ value: m.user_id, label: m.display_name }))]} /></Field>
            )} />
          </div>
          <div className="flex justify-end"><Button type="submit" variant="primary" loading={isSubmitting} disabled={!isDirty}>Save changes</Button></div>
        </form>
      </Card>

      <Card>
        <CardHeader title={archived ? "Archived" : "Archive project"} description={archived ? "This project is read-only. Restore it to work in it again." : "Archiving makes the project read-only and hides it from the active list. Nothing is deleted."} />
        <div className="flex flex-wrap items-center gap-2 p-4">
          {archived ? (
            <Button loading={actions.restore.isPending} onClick={() => actions.restore.mutate(undefined, { onSuccess: () => toast.success("Project restored"), onError: fail("Couldn't restore project") })}>Restore project</Button>
          ) : (
            <Button loading={actions.archive.isPending} onClick={() => actions.archive.mutate(undefined, { onSuccess: () => toast.success("Project archived"), onError: fail("Couldn't archive project") })}>Archive project</Button>
          )}
          {archived && can(orgRole, "manageOrganization") && (
            <Button
              variant="danger"
              loading={actions.remove.isPending}
              onClick={() => {
                if (!window.confirm(`Delete ${project.name}? Its issues will no longer be accessible.`)) return;
                actions.remove.mutate(undefined, { onSuccess: () => { toast.success("Project deleted"); navigate("/projects"); }, onError: fail("Couldn't delete project") });
              }}
            >
              <Trash2 className="h-3.5 w-3.5" aria-hidden /> Delete project
            </Button>
          )}
        </div>
      </Card>
    </div>
  );
}

function Access({ project }: { project: Project }) {
  const access = useProjectAccess(project.key);
  const { set, remove } = useProjectMemberMutations(project.key);
  const { setAccess } = useProjectAction(project.key);
  const { members } = useUserDirectory();
  const [userId, setUserId] = useState("");
  const [role, setRole] = useState<ProjectRole>("member");
  const existing = new Set(access.data?.members.map((m) => m.user_id));
  const candidates = members.filter((m) => m.status === "active" && !existing.has(m.user_id));

  return (
    <div className="space-y-5">
      <Card>
        <CardHeader title="Who can access this project" description="Workspace owners and admins can always administer every project." />
        <div className="p-4">
          <Field label="Everyone in the workspace" hint="Guests are never included; add them by name below.">
            <Select
              className="max-w-xs"
              value={project.default_role ?? "none"}
              disabled={setAccess.isPending}
              onValueChange={(v) => setAccess.mutate(v === "none" ? null : (v as ProjectRole), { onSuccess: () => toast.success("Access updated"), onError: fail("Couldn't update access") })}
              options={[{ value: "none", label: "No access (private project)" }, { value: "member", label: "Can work on issues" }, { value: "viewer", label: "Can view only" }]}
            />
          </Field>
        </div>
      </Card>

      <Card>
        <CardHeader title="People" description="Individual access, in addition to the workspace setting above." />
        <form
          className="flex flex-wrap items-end gap-2 border-b border-line p-4"
          onSubmit={(e: FormEvent) => {
            e.preventDefault();
            if (!userId) return;
            set.mutate({ userId, role }, { onSuccess: () => { setUserId(""); toast.success("Access granted"); }, onError: fail("Couldn't add person") });
          }}
        >
          <div className="w-64 max-w-full">
            <Select aria-label="Person to add" value={userId || "pick"} onValueChange={(v) => setUserId(v === "pick" ? "" : v)} options={[{ value: "pick", label: candidates.length ? "Choose a person…" : "Everyone has been added" }, ...candidates.map((m) => ({ value: m.user_id, label: m.display_name, description: m.email }))]} />
          </div>
          <div className="w-44"><Select aria-label="Role" value={role} onValueChange={(v) => setRole(v as ProjectRole)} options={ROLE_OPTIONS.map(({ value, label }) => ({ value, label }))} /></div>
          <Button type="submit" variant="primary" disabled={!userId} loading={set.isPending}>Add</Button>
        </form>
        {access.isPending ? (
          <div className="space-y-2 p-4" aria-busy><Skeleton className="h-9" /><Skeleton className="h-9" /></div>
        ) : access.isError ? (
          <ErrorState error={access.error} onRetry={() => void access.refetch()} />
        ) : access.data.members.length === 0 ? (
          <EmptyState icon={Users} title="No one has been added individually" description="Add people above to give them access to this project." />
        ) : (
          <ul className="divide-y divide-line">
            {access.data.members.map((m) => (
              <li key={m.user_id} className="flex items-center gap-3 px-4 py-2">
                <div className="min-w-0 flex-1"><UserChip userId={m.user_id} size="md" /></div>
                <div className="w-44">
                  <Select size="sm" aria-label="Role" value={m.role} onValueChange={(v) => set.mutate({ userId: m.user_id, role: v as ProjectRole }, { onError: fail("Couldn't change role") })} options={ROLE_OPTIONS.map(({ value, label }) => ({ value, label }))} />
                </div>
                <Button size="sm" variant="ghost" onClick={() => remove.mutate(m.user_id, { onError: fail("Couldn't remove person") })}>Remove</Button>
              </li>
            ))}
          </ul>
        )}
      </Card>
    </div>
  );
}

function Components({ projectKey }: { projectKey: string }) {
  const components = useComponents(projectKey);
  const { create, remove } = useComponentMutations(projectKey);
  const [name, setName] = useState("");
  return (
    <Card>
      <CardHeader title="Components" description="Sub-sections of a project, such as API, Web or Billing." />
      <form
        className="flex gap-2 border-b border-line p-4"
        onSubmit={(e: FormEvent) => {
          e.preventDefault();
          if (!name.trim()) return;
          create.mutate({ name, description: "" }, { onSuccess: () => setName(""), onError: fail("Couldn't add component") });
        }}
      >
        <Input aria-label="Component name" placeholder="Component name" value={name} maxLength={80} onChange={(e) => setName(e.target.value)} className="max-w-xs" />
        <Button type="submit" variant="primary" disabled={!name.trim()} loading={create.isPending}>Add</Button>
      </form>
      {components.isPending ? (
        <div className="p-4" aria-busy><Skeleton className="h-9" /></div>
      ) : components.isError ? (
        <ErrorState error={components.error} onRetry={() => void components.refetch()} />
      ) : components.data.length === 0 ? (
        <EmptyState icon={Box} title="No components yet" description="Add components to organise issues within this project." />
      ) : (
        <ul className="divide-y divide-line">
          {components.data.map((c) => (
            <li key={c.id} className="flex items-center gap-3 px-4 py-2">
              <span className="flex-1 truncate text-base text-ink">{c.name}</span>
              <Button size="sm" variant="ghost" aria-label={`Delete ${c.name}`} onClick={() => remove.mutate(c.id, { onError: fail("Couldn't delete component") })}>Delete</Button>
            </li>
          ))}
        </ul>
      )}
    </Card>
  );
}

function Releases({ projectKey }: { projectKey: string }) {
  const versions = useVersions(projectKey);
  const { create, setReleased, remove } = useVersionMutations(projectKey);
  const [name, setName] = useState("");
  const [date, setDate] = useState("");
  return (
    <Card>
      <CardHeader title="Releases" description="Versions of your product that issues ship in." />
      <form
        className="flex flex-wrap gap-2 border-b border-line p-4"
        onSubmit={(e: FormEvent) => {
          e.preventDefault();
          if (!name.trim()) return;
          create.mutate({ name, release_date: date || null }, { onSuccess: () => { setName(""); setDate(""); }, onError: fail("Couldn't add version") });
        }}
      >
        <Input aria-label="Version name" placeholder="Version name, e.g. 1.2.0" value={name} maxLength={80} onChange={(e) => setName(e.target.value)} className="max-w-xs" />
        <Input aria-label="Release date" type="date" value={date} onChange={(e) => setDate(e.target.value)} className="w-44" />
        <Button type="submit" variant="primary" disabled={!name.trim()} loading={create.isPending}>Add</Button>
      </form>
      {versions.isPending ? (
        <div className="p-4" aria-busy><Skeleton className="h-9" /></div>
      ) : versions.isError ? (
        <ErrorState error={versions.error} onRetry={() => void versions.refetch()} />
      ) : versions.data.length === 0 ? (
        <EmptyState icon={Rocket} title="No versions yet" description="Create a version to plan what ships when." />
      ) : (
        <ul className="divide-y divide-line">
          {versions.data.map((v) => (
            <li key={v.id} className="flex items-center gap-3 px-4 py-2">
              <span className="min-w-0 flex-1 truncate text-base text-ink">{v.name}</span>
              <span className="hidden text-sm text-ink-muted sm:inline">{v.release_date ? formatDate(v.release_date) : "No date"}</span>
              <Badge tone={v.status === "released" ? "success" : "neutral"}>{v.status === "released" ? "Released" : "Unreleased"}</Badge>
              <Button size="sm" onClick={() => setReleased.mutate({ id: v.id, released: v.status !== "released" }, { onError: fail("Couldn't update version") })}>
                {v.status === "released" ? "Unrelease" : "Release"}
              </Button>
              <Button size="sm" variant="ghost" aria-label={`Delete ${v.name}`} onClick={() => remove.mutate(v.id, { onError: fail("Couldn't delete version") })}>Delete</Button>
            </li>
          ))}
        </ul>
      )}
    </Card>
  );
}
