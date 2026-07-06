import 'server-only';
import fs from 'fs';
import path from 'path';
import initSqlJs, { Database } from 'sql.js';

export interface RegistryItem {
  id: string;
  name: string;
  content_type: string;
  download_strategy: string;
  source_identifier: string;
  sha256: string;
  icon_url: string | null;
  gallery_urls: string[];
  curator_note: string;
  compatible_versions: { mc_version: string; loader: string; mod_version: string }[];
  net_score: number;
  velocity: number;
  is_immune: number;
  status: string;
  allow_comments: number;
  date_added: string | null;
  categories: string[];
}

export interface CuratorReview {
  item_id: string;
  curator_note: string;
  top_reviews_json: string;
}

type SqlJsStatic = Awaited<ReturnType<typeof initSqlJs>>;

let dbPromise: Promise<SqlJsStatic> | null = null;

function getDbPath(): string {
  const fromEnv = process.env.REGISTRY_DB_PATH;
  if (fromEnv) return path.resolve(fromEnv);
  // During `next build`, cwd is the web/ folder, so the compiled DB sits next to it.
  return path.join(process.cwd(), '..', 'registry.db');
}

async function getDb(): Promise<SqlJsStatic> {
  if (!dbPromise) {
    dbPromise = initSqlJs();
  }
  return dbPromise;
}

async function openRegistry(): Promise<Database> {
  const SQL = await getDb();
  const dbPath = getDbPath();
  if (!fs.existsSync(dbPath)) {
    throw new Error(`registry.db not found at ${dbPath}. Run "python compiler/compile.py --skip-sign --out registry.db" first.`);
  }
  const buffer = fs.readFileSync(dbPath);
  return new SQL.Database(buffer);
}

function parseJson<T>(raw: string | null | undefined, fallback: T): T {
  if (!raw) return fallback;
  try {
    return JSON.parse(raw) as T;
  } catch {
    return fallback;
  }
}

function rowToItem(row: Record<string, any>): RegistryItem {
  return {
    id: row.id,
    name: row.name,
    content_type: row.content_type,
    download_strategy: row.download_strategy,
    source_identifier: row.source_identifier,
    sha256: row.sha256,
    icon_url: row.icon_url ?? null,
    gallery_urls: parseJson<string[]>(row.gallery_urls_json, []),
    curator_note: row.curator_note ?? '',
    compatible_versions: parseJson(row.compatible_versions_json, []),
    net_score: row.net_score ?? 0,
    velocity: row.velocity ?? 0,
    is_immune: row.is_immune ?? 0,
    status: row.status ?? 'active',
    allow_comments: row.allow_comments ?? 1,
    date_added: row.date_added ?? null,
    categories: [],
  };
}

function queryAll(db: Database, sql: string, params?: (string | number | null)[]): any[] {
  const stmt = db.prepare(sql);
  if (params) stmt.bind(params);
  const rows: any[] = [];
  while (stmt.step()) {
    rows.push(stmt.getAsObject());
  }
  stmt.free();
  return rows;
}

function queryOne(db: Database, sql: string, params?: (string | number | null)[]): any | null {
  const rows = queryAll(db, sql, params);
  return rows[0] ?? null;
}

export async function getAllItems(contentType?: string): Promise<RegistryItem[]> {
  const db = await openRegistry();
  try {
    const sql = contentType
      ? `SELECT * FROM registry_items WHERE content_type = ? ORDER BY net_score DESC, name ASC`
      : `SELECT * FROM registry_items ORDER BY net_score DESC, name ASC`;
    const rows = contentType ? queryAll(db, sql, [contentType]) : queryAll(db, sql);
    const items = rows.map(rowToItem);

    // Attach categories.
    for (const item of items) {
      const cats = queryAll(
        db,
        `SELECT c.id FROM categories c JOIN item_categories ic ON c.id = ic.category_id WHERE ic.item_id = ?`,
        [item.id]
      );
      item.categories = cats.map((c) => c.id as string);
    }
    return items;
  } finally {
    db.close();
  }
}

export async function getItemById(id: string): Promise<RegistryItem | null> {
  const db = await openRegistry();
  try {
    const row = queryOne(db, `SELECT * FROM registry_items WHERE id = ?`, [id]);
    if (!row) return null;
    const item = rowToItem(row);
    const cats = queryAll(
      db,
      `SELECT c.id FROM categories c JOIN item_categories ic ON c.id = ic.category_id WHERE ic.item_id = ?`,
      [item.id]
    );
    item.categories = cats.map((c) => c.id as string);
    return item;
  } finally {
    db.close();
  }
}

export async function getItemIds(contentType?: string): Promise<string[]> {
  const db = await openRegistry();
  try {
    const sql = contentType
      ? `SELECT id FROM registry_items WHERE content_type = ? ORDER BY id`
      : `SELECT id FROM registry_items ORDER BY id`;
    const rows = contentType ? queryAll(db, sql, [contentType]) : queryAll(db, sql);
    return rows.map((r) => r.id as string);
  } finally {
    db.close();
  }
}

export const CONTENT_TYPES = [
  'mod',
  'pack',
  'shader',
  'resourcepack',
  'server',
  'datapack',
  'world',
] as const;

export type ContentType = (typeof CONTENT_TYPES)[number];

export function isContentType(value: string): value is ContentType {
  return CONTENT_TYPES.includes(value as ContentType);
}

export function contentTypeLabel(type: ContentType): string {
  switch (type) {
    case 'mod':
      return 'Mods';
    case 'pack':
      return 'Modpacks';
    case 'shader':
      return 'Shaders';
    case 'resourcepack':
      return 'Resource Packs';
    case 'server':
      return 'Servers';
    case 'datapack':
      return 'Datapacks';
    case 'world':
      return 'Worlds';
    default:
      return type;
  }
}

export function contentTypePath(type: ContentType): string {
  return `/${type}s`;
}

export function contentTypeFromPath(pathSegment: string): ContentType | null {
  for (const t of CONTENT_TYPES) {
    if (pathSegment === `${t}s`) return t;
  }
  return null;
}


// â”€â”€ Reviews â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

export async function getReviews(itemId: string): Promise<{ author: string; rating: number; body: string; created_at: string }[]> {
  const db = await openRegistry();
  try {
    const row = queryOne(db, `SELECT top_reviews_json FROM curator_reviews WHERE item_id = ?`, [itemId]);
    if (!row || !row.top_reviews_json) return [];
    try {
      const reviews: unknown[] = JSON.parse(row.top_reviews_json);
      return reviews
        .filter((r: unknown): r is Record<string, unknown> => typeof r === 'object' && r !== null && typeof (r as Record<string, unknown>).author === 'string')
        .map((r: Record<string, unknown>) => ({
          author: String(r.author ?? 'Anonymous'),
          rating: typeof r.rating === 'number' ? r.rating : 0,
          body: String(r.body ?? ''),
          created_at: String(r.created_at ?? ''),
        }));
    } catch {
      return [];
    }
  } finally {
    db.close();
  }
}

