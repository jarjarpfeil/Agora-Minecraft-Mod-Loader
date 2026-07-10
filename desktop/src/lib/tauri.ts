import { invoke } from '@tauri-apps/api/core';

/**
 * Format any thrown error (including Tauri's serialized LauncherError shape)
 * into a readable string for UI display.
 *
 * Tauri invokes reject with error values that depend on the Rust enum's serde
 * serialization:
 *   - Unit variants like `HashMismatch` come across as the string `"HashMismatch"`.
 *   - Struct variants like `Generic { code, message }` come across as
 *     `{ Generic: { code: "...", message: "..." } }` (serde's default
 *     externally-tagged representation).
 *
 * Plain JS `Error` objects also flow through here (`e.message` works).
 *
 * Using this helper instead of `String(e)` avoids the dreaded `[object Object]`.
 */
export function formatError(e: unknown): string {
  if (e == null) return 'Unknown error';
  if (typeof e === 'string') return e;
  if (e instanceof Error) return e.message;
  if (typeof e === 'object') {
    const obj = e as Record<string, unknown>;
    // New structured error envelope: { code, message, details, suggested_action }
    if (typeof obj.code === 'string' && typeof obj.message === 'string') {
      return obj.message;
    }
    // Tauri serialized struct variant: { VariantName: { code, message } }
    for (const key of Object.keys(obj)) {
      const inner = obj[key];
      if (inner && typeof inner === 'object') {
        const innerObj = inner as Record<string, unknown>;
        if (typeof innerObj.message === 'string') return innerObj.message;
        if (typeof innerObj.code === 'string') return innerObj.code;
      }
      if (typeof inner === 'string') return inner;
    }
    // Direct shape: { message: "..." } or { code: "..." }
    if (typeof obj.message === 'string') return obj.message;
    if (typeof obj.code === 'string') return obj.code;
    try {
      return JSON.stringify(e);
    } catch {
      return '[object]';
    }
  }
  return String(e);
}

/** Check whether a thrown error is an expired-GitHub-session error. */
export function isAuthExpired(e: unknown): boolean {
  if (e == null || typeof e !== 'object') return false;
  const obj = e as Record<string, unknown>;
  if (obj.code === 'ERR_AUTH_EXPIRED') return true;
  // Tauri serialized struct variant: { AuthExpired: { code: "ERR_AUTH_EXPIRED", ... } }
  for (const key of Object.keys(obj)) {
    const inner = obj[key];
    if (inner && typeof inner === 'object') {
      const innerObj = inner as Record<string, unknown>;
      if (innerObj.code === 'ERR_AUTH_EXPIRED') return true;
    }
  }
  return false;
}

export interface InstanceRow {
  instance_id: string;
  name: string;
  minecraft_version: string;
  loader: string;
  loader_version: string;
  is_modpack: boolean;
  is_locked: boolean;
  last_launched_at: string | null;
  jvm_memory_mb: number;
  jvm_gc: string;
  jvm_custom_args: string;
  created_at: string;
}

export interface InstalledMod {
  filename: string;
  registry_id: string | null;
  modrinth_id: string | null;
  source: string;
  version: string | null;
  sha256: string;
  installed_at: string;
  enabled: boolean;
  content_type: string;
}

export interface InstanceManifest {
  instance_id: string;
  name: string;
  created_from_pack: string | null;
  minecraft_version: string;
  loader: string;
  loader_version: string;
  is_locked: boolean;
  mods: InstalledMod[];
  resourcepacks: InstalledMod[];
  shaders: InstalledMod[];
  datapacks: InstalledMod[];
  worlds: InstalledMod[];
  user_preferences: Record<string, unknown>;
}

export interface InstanceDetail {
  row: InstanceRow;
  manifest: InstanceManifest | null;
}

export interface LoaderVersionSummary {
  loader: string;
  mc_version: string;
  loader_version: string;
  file_type: string;
}

export interface RegistryItem {
  id: string;
  name: string;
  content_type: string;
  download_strategy: string;
  source_identifier: string;
  sha256: string;
  upvotes: number;
  downvotes: number;
  net_score: number;
  velocity: number;
  status: string;
  is_immune: boolean;
  immunity_reason: string | null;
  allow_comments: boolean;
  icon_url: string | null;
  gallery_urls_json: string | null;
  date_added: string | null;
  compatible_versions_json: string | null;
  description: string | null;
  body_markdown: string | null;
  page_url: string | null;
  license_id: string | null;
  source_updated_at: string | null;
  modrinth_id: string | null;
}

export interface CategoryInfo {
  id: string;
  display_name: string;
  is_community: boolean;
}

export type SortOption = 'for_you' | 'net_score' | 'velocity' | 'most_downvoted' | 'newest' | 'most_upvoted';

export interface RegistryStatus {
  has_cached_db: boolean;
  cached_tag: string | null;
  cached_schema_version: number | null;
  latest_tag: string | null;
  update_available: boolean;
  checked: boolean;
  message: string;
}

export interface ExtractionResult {
  extracted: string[];
  skipped: string[];
  total_bytes_written: number;
}

export interface CreateInstanceRequest {
  name: string;
  instance_id: string;
  minecraft_version: string;
  loader: string;
  loader_version: string;
  jvm_memory_mb?: number;
  jvm_gc?: string;
  jvm_custom_args?: string;
}

export interface PackModRow {
  pack_id: string;
  mod_id: string;
  source: string;
  version: string | null;
  status: string;
  description: string | null;
}

export const listPackMods = (packId: string) =>
  invoke<PackModRow[]>('list_pack_mods', { packId });

export const listInstances = () => invoke<InstanceRow[]>('list_instances');
export const getInstanceDetail = (instanceId: string) =>
  invoke<InstanceDetail | null>('get_instance_detail', { instanceId });
export const createInstance = (request: CreateInstanceRequest) =>
  invoke<InstanceRow>('create_instance', { request });
export const deleteInstance = (instanceId: string) =>
  invoke<void>('delete_instance', { instanceId });
export const unlockInstance = (instanceId: string) =>
  invoke<void>('unlock_instance', { instanceId });
export const lockInstance = (instanceId: string) =>
  invoke<void>('lock_instance', { instanceId });
export const renameInstance = (instanceId: string, newName: string) =>
  invoke<void>('rename_instance', { instanceId, newName });
export const revertInstance = (instanceId: string) =>
  invoke<void>('revert_instance', { instanceId });
export const launchInstance = (instanceId: string) =>
  invoke<void>('launch_instance', { instanceId });

export const launchInstanceDirect = (instanceId: string) =>
  invoke<number>('launch_instance_direct', { instanceId });

export const killProcess = (pid: number) =>
  invoke<void>('kill_process', { pid });

export type HealthScore = 'green' | 'yellow' | 'red';

export interface HealthWarning {
  kind: string;
  mod_id: string | null;
  message: string;
  suggested_action: string | null;
}

export interface HealthBlocker {
  kind: string;
  mod_id: string | null;
  message: string;
  suggested_action: string | null;
}

export interface HealthReport {
  score: HealthScore;
  warnings: HealthWarning[];
  blockers: HealthBlocker[];
}

export const checkInstanceHealth = (instanceId: string) =>
  invoke<HealthReport>('check_instance_health', { instanceId });
export const listLoaderVersions = (loader: string, mcVersion: string) =>
  invoke<LoaderVersionSummary[]>('list_loader_versions', {
    loader,
    mcVersion,
  });
export const listManifestLoaders = () =>
  invoke<string[]>('list_manifest_loaders');
export const listManifestMcVersions = (loader?: string) =>
  invoke<string[]>('list_manifest_mc_versions', { loader });
export const forYouItems = (
  modrinthEnabled?: boolean,
  mcVersion?: string,
  loader?: string,
  limit?: number,
  modrinthCategories?: string[],
) =>
  invoke<RegistryItem[]>('for_you_items', {
    modrinthEnabled,
    mcVersion,
    loader,
    limit,
    modrinthCategories,
  });

export const browseItems = (
  contentType?: string,
  category?: string,
  sort?: SortOption,
  modrinthEnabled?: boolean,
  mcVersion?: string,
  loader?: string,
  limit?: number,
) =>
  invoke<RegistryItem[]>('browse_items', {
    contentType,
    category,
    sort,
    modrinthEnabled,
    mcVersion,
    loader,
    limit,
  });
export const getRegistryItem = (itemId: string) =>
  invoke<RegistryItem | null>('get_registry_item', { itemId });
export const listCategories = () => invoke<CategoryInfo[]>('list_categories');

// --- Governance / Transparency Log ---

/**
 * AuditLogEntry — mirrors the Rust `AuditLogEntry` struct in
 * desktop/src-tauri/src/registry.rs. Keep these two definitions in sync:
 * adding/removing/renaming a field on the Rust struct requires the same change
 * here, or the value will be silently dropped at the IPC boundary.
 * TODO: replace this hand-mirror with generated types (e.g. ts-rs) once a
 * codegen step is wired into the build.
 */
export interface AuditLogEntry {
  id: number;
  timestamp: string;
  action: string;
  details: string | null;
}

export const listAuditLog = (limit?: number) =>
  invoke<AuditLogEntry[]>('list_audit_log', { limit });
export const checkRegistryUpdate = (force?: boolean) =>
  invoke<RegistryStatus>('check_registry_update', { force });
export const getRegistryStatus = () => invoke<RegistryStatus>('get_registry_status');
export const extractOverrides = (zipPath: string, instanceId: string) =>
  invoke<ExtractionResult>('extract_overrides', { zipPath, instanceId });
export const getSetting = (key: string) =>
  invoke<unknown | null>('get_setting', { key });
export const setSetting = (key: string, value: unknown) =>
  invoke<void>('set_setting', { key, value });

export interface DeviceFlowResponse {
  device_code: string;
  user_code: string;
  verification_uri: string;
  expires_in: number;
  interval: number;
}

export interface GithubProfile {
  login: string;
  avatar_url: string;
}

export const githubLogin = () => invoke<DeviceFlowResponse>('github_login');
export const githubLoginPoll = (deviceCode: string, interval: number) =>
  invoke<boolean>('github_login_poll', { deviceCode, interval });
export const githubLogout = () => invoke<void>('github_logout');
export const getAuthStatus = () => invoke<boolean>('get_auth_status');
export const getGithubProfile = () =>
  invoke<GithubProfile | null>('get_github_profile');

export interface CrashReportInfo {
  filename: string;
  modified_at: string;
  size_bytes: number;
}

export interface CrashTriageResult {
  matched: boolean;
  signature_name: string | null;
  solution_markdown: string | null;
  action_button_json: string | null;
}

export const checkInstanceCrash = (instanceId: string) =>
  invoke<CrashReportInfo | null>('check_instance_crash', { instanceId });
export const triageCrashReport = (instanceId: string, filename: string) =>
  invoke<CrashTriageResult>('triage_crash_report', { instanceId, filename });
export const listCrashReports = (instanceId: string) =>
  invoke<CrashReportInfo[]>('list_crash_reports_cmd', { instanceId });
export const readCrashLog = (instanceId: string, filename: string) =>
  invoke<string>('read_crash_log_cmd', { instanceId, filename });

export interface ModVersionCandidate {
  version: string;
  filename: string;
  download_url: string;
  mc_version: string | null;
  loader: string | null;
  release_date: string | null;
  is_compatible: boolean;
  sha1?: string | null;
  version_compat?: string;
}

export interface ModVersionPage {
  items: ModVersionCandidate[];
  hasMore: boolean;
  total: number;
}

export const listModVersions = (instanceId: string | null, itemId: string) =>
  invoke<ModVersionPage>('list_mod_versions', { instanceId, itemId });

export const listModVersionsLoadMore = (instanceId: string | null, itemId: string, page: number) =>
  invoke<ModVersionPage>('list_mod_versions_load_more', { instanceId, itemId, page });

/// Quick compat probe for the browse page — returns
/// `"compatible"`, `"major_match"`, or `""` (incompatible).
export const checkModCompat = (instanceId: string, itemId: string) =>
  invoke<string>('check_mod_compat', { instanceId, itemId });

export const installModVersion = (
  instanceId: string,
  itemId: string,
  candidate: ModVersionCandidate,
) => invoke<InstalledMod>('install_mod_version', { instanceId, itemId, candidate });

export const removeModFromInstance = (instanceId: string, filename: string) =>
  invoke<void>('remove_mod_from_instance', { instanceId, filename });
export const disableInstanceMod = (instanceId: string, filename: string) =>
  invoke<void>('disable_instance_mod', { instanceId, filename });
export const enableInstanceMod = (instanceId: string, filename: string) =>
  invoke<void>('enable_instance_mod', { instanceId, filename });

export const addManualMod = (instanceId: string, sourcePath: string) =>
  invoke<InstalledMod>('add_manual_mod', { instanceId, sourcePath });

export const exportInstancePack = (instanceId: string, format: 'json' | 'mrpack') =>
  invoke<string>('export_instance_pack', { instanceId, format });

export const pickOpenFile = (title: string, extensions: string[]) =>
  invoke<string | null>('pick_open_file', { title, extensions });

export const importInstancePack = (sourcePath: string) =>
  invoke<string>('import_instance_pack', { sourcePath });
export const importModrinthPackByUrl = (downloadUrl: string) =>
  invoke<string>('import_modrinth_pack_by_url', { downloadUrl });

// --- Raw (uncurated) Modrinth integration (§6.3) ---

export interface ModrinthSearchResult {
  project_id: string;
  slug: string;
  title: string;
  description: string;
  icon_url: string | null;
  author: string;
  categories: string[];
  downloads: number;
  follows: number;
  project_type: string;
  date_created: string | null;
  date_modified: string | null;
  versions: string[];
  license: string | null;
}

export type ModrinthSort = 'relevance' | 'follows' | 'newest' | 'updated';

export interface ModrinthSearchParams {
  query?: string;
  categories?: string[];
  loaders?: string[];
  game_versions?: string[];
  project_type?: string;
  sort?: ModrinthSort;
  offset?: number;
  limit?: number;
}

export interface ModrinthSearchPage {
  results: ModrinthSearchResult[];
  total_hits: number;
  offset: number;
  limit: number;
}

export interface ModrinthCategoryInfo {
  name: string;
  project_type: string;
  header: string;
}

export interface ModrinthLoaderInfo {
  name: string;
  supported_project_types: string[];
}

export interface ModrinthGameVersionInfo {
  version: string;
  version_type: string;
  date: string;
  major: boolean;
}

export interface RawModrinthVersionCandidate {
  version: string;
  version_id: string;
  name: string;
  filename: string;
  download_url: string;
  sha1: string | null;
  mc_versions: string[];
  loaders: string[];
  release_date: string | null;
  primary: boolean;
  changelog: string | null;
}

export const isModrinthEnabled = () => invoke<boolean>('is_modrinth_enabled');
export const searchModrinth = (params: ModrinthSearchParams) =>
  invoke<ModrinthSearchPage>('search_modrinth', { params });
export const listModrinthCategories = () =>
  invoke<ModrinthCategoryInfo[]>('list_modrinth_categories');
export const listModrinthLoaders = () =>
  invoke<ModrinthLoaderInfo[]>('list_modrinth_loaders');
export const listModrinthGameVersions = () =>
  invoke<ModrinthGameVersionInfo[]>('list_modrinth_game_versions');
export const listRawModrinthVersions = (instanceId: string | null, projectId: string, projectType?: string) =>
  invoke<RawModrinthVersionCandidate[]>('list_raw_modrinth_versions', {
    instanceId,
    projectId,
    projectType: projectType ?? null,
  });
export const installRawModrinth = (
  instanceId: string,
  projectId: string,
  candidate: RawModrinthVersionCandidate,
  projectType?: string,
) => invoke<InstalledMod>('install_raw_modrinth', { instanceId, projectId, candidate, projectType: projectType ?? null });

export interface ModrinthProjectFull {
    id: string;
    title: string;
    description: string;
    body: string | null;
    icon_url: string | null;
    project_type: string;
    page_url: string | null;
    license_id: string | null;
    source_updated_at: string | null;
    gallery_urls: string[];
}

export const fetchModrinthProject = (projectId: string) =>
  invoke<ModrinthProjectFull>('fetch_modrinth_project', { projectId });

// --- Phase 7: Curated annotation overlay for Modrinth projects ---

export interface CuratedAnnotation {
  id: string;
  name: string;
  curator_note: string | null;
  net_score: number | null;
  is_immune: boolean;
  base_categories: string[];
}

export const getCuratedAnnotation = (modrinthId: string) =>
  invoke<CuratedAnnotation | null>('get_curated_annotation', { modrinthId });

// --- Governance / Triage ---

export interface UnderReviewItem {
  id: string;
  name: string;
  content_type: string;
  icon_url: string | null;
  net_score: number;
}

export interface ModReview {
  author: string | null;
  text: string;
  issue_number: number;
  created_at: string | null;
}

export interface TriagePoll {
  discussion_url: string | null;
  keep_votes: number;
  remove_votes: number;
}

export interface FlagRateLimit {
  remaining_hour: number;
  remaining_day: number;
  reset_hour_at_unix: number;
  reset_day_at_unix: number;
  can_flag: boolean;
}

export const listUnderReviewItems = () =>
  invoke<UnderReviewItem[]>('list_under_review_items');

export const listRecentResolutions = (limit?: number) =>
  invoke<AuditLogEntry[]>('list_recent_resolutions', { limit });

export const listModReviews = (itemId: string) =>
  invoke<ModReview[]>('list_mod_reviews', { itemId });

export const fetchTriagePoll = (modId: string) =>
  invoke<TriagePoll>('fetch_triage_poll', { modId });

export const flagReview = (params: {
  modId: string;
  modName: string;
  issueNumber: number;
  author: string;
  quotedText: string;
  reporterLogin: string;
}) => invoke<string>('flag_review', params);

export const getFlagRateLimit = () =>
  invoke<FlagRateLimit>('get_flag_rate_limit');

// --- Crash Investigation (guided isolation) ---

export interface CrashFingerprint {
  exception_class: string;
  top_frames: string[];
}

export interface SuspectScore {
  mod_id: string;
  filename: string;
  total_score: number;
  breakdown: Record<string, unknown>;
  is_dependent_of: string | null;
}

export type SuggestedAction =
  | { kind: 'GuidedDisable'; next_suspect: SuspectScore }
  | { kind: 'ConfidenceAutoDisable'; mod_id: string; filename: string }
  | { kind: 'ShowTriageBanner'; mod_id: string }
  | { kind: 'NoSuspects' };

export interface InvestigationResult {
  fingerprint: CrashFingerprint | null;
  signature_name: string | null;
  suspects: SuspectScore[];
  suggested_action: SuggestedAction;
  ruled_out: string[];
}

export const investigateCrash = (instanceId: string, filename?: string) =>
  invoke<InvestigationResult>('investigate_crash', { instanceId, filename });

export const investigateManual = (instanceId: string, logText: string) =>
  invoke<InvestigationResult>('investigate_manual', { instanceId, logText });

export const disableModForTest = (instanceId: string, filename: string) =>
  invoke<void>('disable_mod_for_test', { instanceId, filename });

export const enableModForTest = (instanceId: string, filename: string) =>
  invoke<void>('enable_mod_for_test', { instanceId, filename });

export const confirmCrashFix = (fingerprint: CrashFingerprint, modId: string) =>
  invoke<void>('confirm_crash_fix', { fingerprint, modId });

export const reportStillCrashing = (
  instanceId: string,
  fingerprint: CrashFingerprint,
  ruledOutModId: string,
  crashLogText: string,
) =>
  invoke<InvestigationResult>('report_still_crashing', {
    instanceId,
    fingerprint,
    ruledOutModId,
    crashLogText,
  });

// --- Dependency Plans (PREVIEW) ---

export type Requirement = 'Required' | 'Optional';

export type DepSource = 'Jar' | 'Manifest';

export interface DependentInfo {
  mod_id: string;
  filename: string;
  requirement: Requirement;
  source: DepSource;
}

export interface DepCandidate {
  mod_jar_id: string;
  requirement: Requirement;
  source: DepSource;
}

export interface DepConflict {
  mod_jar_id: string;
  jar_requirement: Requirement | null;
  manifest_requirement: Requirement | null;
}

export interface InstallPlan {
  missing_required: DepCandidate[];
  missing_optional: DepCandidate[];
  conflicts: DepConflict[];
}

export interface RemovalPlan {
  dependents: DependentInfo[];
}

export interface DisablePlan {
  dependents: DependentInfo[];
}

export const getDisablePlan = (instanceId: string, filename: string) =>
  invoke<DisablePlan>('get_disable_plan', { instanceId, filename });

export const getRemovalPlan = (instanceId: string, filename: string) =>
  invoke<RemovalPlan>('get_removal_plan', { instanceId, filename });

export const getInstallPlan = (instanceId: string, itemId: string, jarPath: string) =>
  invoke<InstallPlan>('get_install_plan', { instanceId, itemId, jarPath });

export const enableModWithAutoDeps = (instanceId: string, filename: string) =>
  invoke<string[]>('enable_mod_with_auto_deps', { instanceId, filename });

// --- MCP Server Lifecycle ---

export interface McpStatus {
  running: boolean;
  url: string | null;
}

export const startMcpServer = () => invoke<McpStatus>('start_mcp_server');
export const stopMcpServer = () => invoke<void>('stop_mcp_server');
export const getMcpStatus = () => invoke<McpStatus>('get_mcp_status');
export const getMcpSkillContent = () => invoke<string>('get_mcp_skill_content');
export const setMcpApproval = (toolName: string, instanceId: string, state: string) =>
  invoke<void>('set_mcp_approval', { toolName, instanceId, state });

// --- MCP Token Management ---

export interface McpTokenData {
  token: string;
  config_snippet: string;
}

export const getMCPToken = () => invoke<McpTokenData>('get_mcp_token');
export const regenerateMCPToken = () => invoke<McpTokenData>('regenerate_mcp_token');

// --- AI Assistant (GitHub Models) ---

export interface ChatMessage {
  role: string;
  content: string;
}

export interface ChatResponse {
  content: string;
  model: string;
}

export interface AiContext {
  instance_id: string | null;
  crash_log: string | null;
  crash_signatures: string | null;
  suspects: string | null;
}

export interface AvailableModel {
    id: string;
    name: string;
    description: string;
    free_tier: boolean;
}

export const aiChat = (
  messages: ChatMessage[],
  context?: AiContext | null,
) =>
  invoke<ChatResponse>('ai_chat', {
    messages,
    context: context ?? null,
  });

export const aiGetModels = () =>
  invoke<AvailableModel[]>('ai_get_models');

export const aiGetDefaultModel = () =>
  invoke<string>('ai_get_default_model');

export const getWindowsAccentColor = () =>
  invoke<string | null>('get_windows_accent_color');

// --- AI Copilot auth ---

export interface CopilotDeviceFlowResponse {
  device_code: string;
  user_code: string;
  verification_uri: string;
  expires_in: number;
  interval: number;
}

export interface CopilotToken {
  access_token: string;
  copilot_token: string | null;
  endpoint: string;
  plan: string;
  username: string;
  stored_at: string;
}

export const copilotLogin = () =>
  invoke<CopilotDeviceFlowResponse>('copilot_login');

/** Try to use the existing governance GitHub token for Copilot, skipping the
 *  device flow if the token works and the user has a Copilot subscription. */
export const copilotTryGovernanceToken = () =>
  invoke<CopilotToken | null>('copilot_try_governance_token');

export const copilotLoginPoll = (deviceCode: string, interval: number) =>
  invoke<CopilotToken>('copilot_login_poll', { deviceCode, interval });

export const copilotStatus = () =>
  invoke<CopilotToken | null>('copilot_status');

export const copilotLogout = () =>
  invoke<void>('copilot_logout');

// ---------------------------------------------------------------------------
// Phase 5: MSA auth + GC architect
// ---------------------------------------------------------------------------

export interface MsaCredentials {
  username: string;
  uuid: string;
  access_token: string;
  refresh_token: string;
  expires: string;
}

export type GcProfile = 'low_latency' | 'high_efficiency' | 'manual';

export interface GcResult {
  profile: GcProfile;
  jvm_args: string;
  heap_mb: number;
  total_ram_mb: number;
  cpu_threads: number;
  recommended: boolean;
}

export const msaBeginLogin = () =>
  invoke<{ auth_uri: string }>('msa_begin_login');

export const msaFinishLogin = (code: string, oauthState?: string | null) =>
  invoke<MsaCredentials>('msa_finish_login', { code, oauthState });

export const msaGetStatus = () =>
  invoke<MsaCredentials | null>('msa_get_status');

export const msaRefresh = () =>
  invoke<MsaCredentials>('msa_refresh');

export const msaLogout = () =>
  invoke<void>('msa_logout');

export const computeGcArgs = (
  javaVersion: number,
  requestedHeapMb: number,
  manualArgs: string,
  overrideProfile?: GcProfile,
) =>
  invoke<GcResult>('compute_gc_args', {
    javaVersion,
    requestedHeapMb,
    manualArgs,
    overrideProfile,
  });

// ---------------------------------------------------------------------------
// Phase 6: Instance lifecycle
// ---------------------------------------------------------------------------

export interface Snapshot {
  id: string;
  label: string | null;
  created_at: string;
  file_count: number;
  size_estimate: number;
}

export interface LoadoutProfile {
  name: string;
  enabled_mods: string[];
  created_at: string;
}

export interface ImportResult {
  instance_id: string;
  name: string;
  minecraft_version: string;
  loader: string;
  loader_version: string;
  imported_mods: number;
  linked_saves: boolean;
}

export interface DetectedLauncher {
  launcher_type: string;
  instances_dir: string;
  instance_count: number;
}

export interface ClonePrefs {
  copy_saves: boolean;
  copy_mods: boolean;
  copy_resource_packs: boolean;
  copy_shader_packs: boolean;
  copy_screenshots: boolean;
  copy_config: boolean;
  copy_servers: boolean;
  copy_options: boolean;
  use_hard_links: boolean;
  use_sym_links: boolean;
}

export interface ExportResult {
  total_mods: number;
  server_mods: number;
  removed_client_only: string[];
  server_jar_downloaded: boolean;
  start_scripts_created: boolean;
}

export const listSnapshots = (instanceId: string) =>
  invoke<Snapshot[]>('list_snapshots', { instanceId });

export const createSnapshot = (instanceId: string, label?: string) =>
  invoke<Snapshot>('create_snapshot', { instanceId, label });

export const restoreSnapshot = (instanceId: string, snapshotId: string) =>
  invoke<void>('restore_snapshot', { instanceId, snapshotId });

export const deleteSnapshot = (instanceId: string, snapshotId: string) =>
  invoke<void>('delete_snapshot', { instanceId, snapshotId });

export const listLoadoutProfiles = (instanceId: string) =>
  invoke<LoadoutProfile[]>('list_loadout_profiles', { instanceId });

export const createLoadoutProfile = (instanceId: string, name: string) =>
  invoke<LoadoutProfile>('create_loadout_profile', { instanceId, name });

export const applyLoadoutProfile = (instanceId: string, profileName: string) =>
  invoke<void>('apply_loadout_profile', { instanceId, profileName });

export const deleteLoadoutProfile = (instanceId: string, profileName: string) =>
  invoke<void>('delete_loadout_profile', { instanceId, profileName });

export const importInstance = (sourcePath: string, symlinkSaves: boolean) =>
  invoke<ImportResult>('import_instance', { sourcePath, symlinkSaves });

export const detectLaunchers = () =>
  invoke<DetectedLauncher[]>('detect_launchers');

export const cloneInstance = (instanceId: string, newName: string, prefs: ClonePrefs) =>
  invoke<string>('clone_instance_cmd', { instanceId, newName, prefs });

export const exportServerEnvironment = (instanceId: string, destPath: string) =>
  invoke<ExportResult>('export_server_environment', { instanceId, destPath });

// ---------------------------------------------------------------------------
// Phase 8: Pack install
// ---------------------------------------------------------------------------

export interface PackManifest {
  name: string;
  minecraft_version: string;
  loader: string;
  loader_version: string;
  mods: PackModEntry[];
  override_source?: OverrideSource;
}

export interface PackModEntry {
  id: string;
  source: string;
  version?: string;
  status: string;
}

export interface OverrideSource {
  type: string;
  identifier: string;
  release_tag: string;
  asset_name: string;
  sha256?: string;
}

export interface PackInstallResult {
  instance_id: string;
  name: string;
  mods_installed: number;
  overrides_extracted: boolean;
}

export const installPack = (manifestJson: string, instanceId: string) =>
  invoke<PackInstallResult>('install_pack', { manifestJson, instanceId });

// --- Browse cache (Rust-backed, paginated) ---

export interface BrowseItemCached {
  id: string;
  source: string;
  registryItem: RegistryItem | null;
  modrinthResult: ModrinthSearchResult | null;
  name: string;
  iconUrl: string | null;
  description: string | null;
  contentType: string;
}

export interface BrowsePage {
  items: BrowseItemCached[];
  total: number;
  page: number;
  hasMore: boolean;
}

export const browseSearch = (
  query?: string,
  contentType?: string,
  category?: string,
  sort?: string,
  mcVersion?: string,
  loader?: string,
) =>
  invoke<BrowsePage>('browse_search', {
    query: query ?? null,
    contentType: contentType ?? null,
    category: category ?? null,
    sort: sort ?? null,
    mcVersion: mcVersion ?? null,
    loader: loader ?? null,
  });

export const browseLoadMore = () =>
  invoke<BrowsePage>('browse_load_more');

export const browsePage = (page: number) =>
  invoke<BrowsePage>('browse_page', { page });
