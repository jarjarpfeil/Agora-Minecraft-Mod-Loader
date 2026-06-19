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

export type SortOption = 'net_score' | 'velocity' | 'most_downvoted' | 'newest' | 'most_upvoted';

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
export const launchInstance = (instanceId: string) =>
  invoke<void>('launch_instance', { instanceId });
export const listLoaderVersions = (loader: string, mcVersion: string) =>
  invoke<LoaderVersionSummary[]>('list_loader_versions', {
    loader,
    mcVersion,
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
}

export const listModVersions = (instanceId: string, itemId: string) =>
  invoke<ModVersionCandidate[]>('list_mod_versions', { instanceId, itemId });

export const installModVersion = (
  instanceId: string,
  itemId: string,
  candidate: ModVersionCandidate,
) => invoke<InstalledMod>('install_mod_version', { instanceId, itemId, candidate });

export const removeModFromInstance = (instanceId: string, filename: string) =>
  invoke<void>('remove_mod_from_instance', { instanceId, filename });

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

export type ModrinthSort = 'relevance' | 'downloads' | 'follows' | 'newest' | 'updated';

export interface ModrinthSearchParams {
  query?: string;
  categories?: string[];
  loaders?: string[];
  game_versions?: string[];
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
export const listRawModrinthVersions = (instanceId: string | null, projectId: string) =>
  invoke<RawModrinthVersionCandidate[]>('list_raw_modrinth_versions', {
    instanceId,
    projectId,
  });
export const installRawModrinth = (
  instanceId: string,
  projectId: string,
  candidate: RawModrinthVersionCandidate,
) => invoke<InstalledMod>('install_raw_modrinth', { instanceId, projectId, candidate });
