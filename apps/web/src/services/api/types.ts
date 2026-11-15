// Types generated from each service's OpenAPI document (`pnpm gen:api`).
// Never hand-edit the generated files; change the Rust DTO and regenerate.
import type { components as AgileComponents } from "./generated/agile";
import type { components as NotificationComponents } from "./generated/notification";
import type { components as SearchComponents } from "./generated/search";
import type { components as IdentityComponents } from "./generated/identity";
import type { components as UserComponents } from "./generated/user";
import type { components as WorkflowComponents } from "./generated/workflow";

import type { components as IssueComponents } from "./generated/issue";
import type { components as ProjectComponents } from "./generated/project";
import type { components as AnalyticsComponents } from "./generated/analytics";

type Identity = IdentityComponents["schemas"];
type User = UserComponents["schemas"];

export type Account = Identity["AccountDto"];
export type TokenResponse = Identity["TokenResponse"];
export type Session = Identity["SessionDto"];
export type SessionList = Identity["SessionList"];
export type RegisterRequest = Identity["RegisterRequest"];
export type LoginRequest = Identity["LoginRequest"];
export type MfaChallenge = Identity["MfaChallenge"];
export type MfaStatus = Identity["MfaStatus"];
export type TotpSetup = Identity["TotpSetup"];
export type RecoveryCodes = Identity["RecoveryCodes"];
export type ApiToken = Identity["ApiTokenDto"];
export type ApiTokenList = Identity["ApiTokenList"];
export type CreateApiToken = Identity["CreateApiToken"];
export type CreatedApiToken = Identity["CreatedApiToken"];

export type Profile = User["ProfileDto"];
export type UpdateProfileRequest = User["UpdateProfileRequest"];
export type Organization = User["OrganizationDto"];
export type OrganizationList = User["OrganizationList"];
export type Member = User["MemberDto"];
export type MemberPage = User["MemberPage"];
export type AuditEntry = User["AuditEntryDto"];
export type AuditPage = User["AuditPage"];
export type AuditVerification = User["AuditVerification"];
export type Invitation = User["InvitationDto"];
export type InvitationList = User["InvitationList"];
export type InvitationPreview = User["InvitationPreview"];
export type AcceptInvitationResponse = User["AcceptInvitationResponse"];

type ProjectApi = ProjectComponents["schemas"];
type WorkflowApi = WorkflowComponents["schemas"];
type IssueApi = IssueComponents["schemas"];

export type Project = ProjectApi["ProjectDto"];
export type ProjectList = ProjectApi["ProjectList"];
export type CreateProjectRequest = ProjectApi["CreateProjectRequest"];
export type UpdateProjectRequest = ProjectApi["UpdateProjectRequest"];
export type Component = ProjectApi["ComponentDto"];
export type ComponentList = ProjectApi["ComponentList"];
export type Version = ProjectApi["VersionDto"];
export type VersionList = ProjectApi["VersionList"];
export type ProjectAccess = ProjectApi["ProjectAccessDto"];
export type ProjectRole = "administrator" | "member" | "viewer";

export type Issue = IssueApi["IssueDto"];
export type IssueDetail = IssueApi["IssueDetail"];
export type IssuePage = IssueApi["IssuePage"];
export type IssueMeta = IssueApi["IssueMeta"];
export type CreateIssueRequest = IssueApi["CreateIssueRequest"];
export type UpdateIssueRequest = IssueApi["UpdateIssueRequest"];
export type IssueComment = IssueApi["CommentDto"];
export type CommentList = IssueApi["CommentList"];
export type IssueLink = IssueApi["LinkDto"];
export type LinkList = IssueApi["LinkList"];
export type HistoryEntry = IssueApi["HistoryEntry"];
export type HistoryList = IssueApi["HistoryList"];
export type CustomField = IssueApi["CustomFieldDto"];
export type CustomFieldList = IssueApi["CustomFieldList"];
export type Attachment = IssueApi["AttachmentDto"];
export type AttachmentList = IssueApi["AttachmentList"];
export type WorkflowSummary = WorkflowApi["WorkflowSummary"];
export type WorkflowDetail = WorkflowApi["WorkflowDetail"];
export type WorkflowList = WorkflowApi["WorkflowList"];
export type WorkflowDefinition = WorkflowApi["Definition"];
export type WorkflowStatus = WorkflowApi["StatusDef"];
export type WorkflowTransition = WorkflowApi["Transition"];
export type WorkflowCondition = WorkflowApi["Condition"];
export type WorkflowValidator = WorkflowApi["Validator"];
export type WorkflowPostFunction = WorkflowApi["PostFunction"];
export type WorkflowProblem = WorkflowApi["Problem"];
export type WorkflowAssignmentList = WorkflowApi["AssignmentList"];
export type WorkflowStatusList = WorkflowApi["StatusList"];
export type IssueTransition = IssueApi["TransitionDto"];
export type TransitionList = IssueApi["TransitionList"];
type AgileApi = AgileComponents["schemas"];
export type Board = AgileApi["BoardDto"];
export type BoardList = AgileApi["BoardList"];
export type BoardColumn = AgileApi["Column"];
export type BoardCard = AgileApi["CardDto"];
export type BoardView = AgileApi["BoardView"];
export type BacklogView = AgileApi["BacklogView"];
export type Sprint = AgileApi["SprintDto"];
export type SprintList = AgileApi["SprintList"];
export type SprintReport = AgileApi["SprintReport"];
export type BurndownSample = AgileApi["BurndownSample"];
export type VelocityReport = AgileApi["VelocityReport"];

type NotificationApi = NotificationComponents["schemas"];
export type AppNotification = NotificationApi["NotificationDto"];
export type NotificationList = NotificationApi["NotificationList"];
export type UnreadCount = NotificationApi["UnreadCount"];
export type NotificationPreferences = NotificationApi["NotificationPreferences"];
export type UnsubscribeLink = NotificationApi["UnsubscribeLink"];

type SearchApi = SearchComponents["schemas"];
export type SearchHit = SearchApi["SearchHit"];
export type SearchResults = SearchApi["SearchResults"];
export type TextPart = SearchApi["TextPart"];
export type SavedFilter = SearchApi["SavedFilter"];
export type MatchingIds = SearchApi["MatchingIds"];
export type SavedFilterList = SearchApi["SavedFilterList"];
export type CreateSavedFilter = SearchApi["CreateSavedFilter"];
export type UpdateSavedFilter = SearchApi["UpdateSavedFilter"];
export type FilterSubscription = SearchApi["Subscription"];
export type PutFilterSubscription = SearchApi["PutSubscription"];
/** A rich-text document in the editor's JSON format. */
type AnalyticsApi = AnalyticsComponents["schemas"];
export type DashboardSummary = AnalyticsApi["DashboardSummary"];
export type DashboardList = AnalyticsApi["DashboardList"];
export type Dashboard = AnalyticsApi["Dashboard"];
export type CreateDashboard = AnalyticsApi["CreateDashboard"];
export type UpdateDashboard = AnalyticsApi["UpdateDashboard"];
export type Widget = AnalyticsApi["Widget"];
export type CreateWidget = AnalyticsApi["CreateWidget"];
export type UpdateWidget = AnalyticsApi["UpdateWidget"];
export type WidgetQuery = AnalyticsApi["WidgetQuery"];
export type WidgetData = AnalyticsApi["WidgetData"];
export type WidgetSize = AnalyticsApi["WidgetSize"];
export type GroupBy = AnalyticsApi["GroupBy"];

export type RichDoc = Record<string, unknown>;

export type OrgRole = "owner" | "admin" | "member" | "guest";
