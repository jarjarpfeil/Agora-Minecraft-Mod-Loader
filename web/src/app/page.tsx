import Link from 'next/link';
import { getAllItems, CONTENT_TYPES, contentTypeLabel, contentTypePath } from '@/lib/db';
import { GITHUB_REPO_URL, GITHUB_RELEASES_URL } from '@/lib/site';

export default async function HomePage() {
  const mods = await getAllItems('mod');
  const packs = await getAllItems('pack');
  const featuredMods = mods.slice(0, 4);
  const featuredPacks = packs.slice(0, 2);

  return (
    <div className="space-y-12">
      <section className="rounded-2xl bg-indigo-600 px-6 py-16 text-center text-white dark:bg-indigo-700">
        <h1 className="text-4xl font-extrabold tracking-tight md:text-5xl">
          Agora Minecraft Mod Launcher
        </h1>
        <p className="mx-auto mt-4 max-w-2xl text-lg text-indigo-100">
          A boutique, community-curated Minecraft mod platform. If CurseForge is a beer, this is Agora.
        </p>
        <div className="mt-8 flex flex-wrap justify-center gap-4">
          <Link
            href="/mods"
            className="rounded-lg bg-white px-5 py-3 font-semibold text-indigo-700 shadow-sm hover:bg-gray-100"
          >
            Browse the database
          </Link>
          <a
            href={GITHUB_RELEASES_URL}
            className="rounded-lg bg-indigo-500 px-5 py-3 font-semibold text-white hover:bg-indigo-400"
            target="_blank"
            rel="noopener noreferrer"
          >
            Install the desktop app
          </a>
          <a
            href={GITHUB_REPO_URL}
            className="rounded-lg border border-white px-5 py-3 font-semibold text-white hover:bg-white/10"
            target="_blank"
            rel="noopener noreferrer"
          >
            View on GitHub
          </a>
        </div>
      </section>

      <section>
        <h2 className="mb-6 text-2xl font-bold">Browse by type</h2>
        <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
          {CONTENT_TYPES.map((type) => (
            <Link
              key={type}
              href={contentTypePath(type as any)}
              className="rounded-xl border bg-white p-5 shadow-sm transition hover:shadow-md dark:border-gray-700 dark:bg-gray-800"
            >
              <h3 className="text-lg font-semibold">{contentTypeLabel(type as any)}</h3>
              <p className="mt-1 text-sm text-gray-600 dark:text-gray-400">
                Curated {type} entries.
              </p>
            </Link>
          ))}
        </div>
      </section>

      {featuredPacks.length > 0 && (
        <section>
          <h2 className="mb-6 text-2xl font-bold">Featured modpacks</h2>
          <div className="grid gap-4 sm:grid-cols-2">
            {featuredPacks.map((pack) => (
              <Link
                key={pack.id}
                href={`/packs/${pack.id}`}
                className="rounded-xl border bg-white p-5 shadow-sm transition hover:shadow-md dark:border-gray-700 dark:bg-gray-800"
              >
                <h3 className="text-lg font-semibold">{pack.name}</h3>
                <p className="mt-2 line-clamp-2 text-sm text-gray-600 dark:text-gray-400">
                  {pack.curator_note}
                </p>
              </Link>
            ))}
          </div>
        </section>
      )}

      {featuredMods.length > 0 && (
        <section>
          <h2 className="mb-6 text-2xl font-bold">Top mods</h2>
          <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
            {featuredMods.map((mod) => (
              <Link
                key={mod.id}
                href={`/mods/${mod.id}`}
                className="rounded-xl border bg-white p-5 shadow-sm transition hover:shadow-md dark:border-gray-700 dark:bg-gray-800"
              >
                <h3 className="font-semibold">{mod.name}</h3>
                <p className="mt-2 line-clamp-3 text-sm text-gray-600 dark:text-gray-400">
                  {mod.curator_note}
                </p>
              </Link>
            ))}
          </div>
        </section>
      )}
    </div>
  );
}
