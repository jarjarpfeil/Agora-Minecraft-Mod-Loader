import Link from 'next/link';
import { CONTENT_TYPES, contentTypeLabel } from '@/lib/db';
import { GITHUB_REPO_URL } from '@/lib/site';

export function Shell({ children }: { children: React.ReactNode }) {
  return (
    <div className="flex min-h-screen flex-col bg-gray-50 text-gray-900 dark:bg-gray-900 dark:text-gray-100">
      <header className="border-b bg-white dark:border-gray-700 dark:bg-gray-800">
        <div className="mx-auto flex max-w-6xl flex-col gap-4 px-6 py-4 md:flex-row md:items-center md:justify-between">
          <Link href="/" className="text-xl font-bold">
            Agora
          </Link>
          <nav className="flex flex-wrap gap-4 text-sm">
            <Link href="/" className="hover:text-indigo-600 dark:hover:text-indigo-400">
              Home
            </Link>
            {CONTENT_TYPES.map((type) => (
              <Link
                key={type}
                href={`/${type}s`}
                className="hover:text-indigo-600 dark:hover:text-indigo-400"
              >
                {contentTypeLabel(type as any)}
              </Link>
            ))}
            <Link href="/about" className="hover:text-indigo-600 dark:hover:text-indigo-400">
              About
            </Link>
          </nav>
        </div>
      </header>

      <main className="mx-auto w-full max-w-6xl flex-1 px-6 py-8">{children}</main>

      <footer className="border-t bg-white px-6 py-6 text-sm text-gray-600 dark:border-gray-700 dark:bg-gray-800 dark:text-gray-400">
        <div className="mx-auto flex max-w-6xl flex-col gap-4 md:flex-row md:items-center md:justify-between">
          <p>Agora — a boutique, community-curated Minecraft mod launcher.</p>
          <div className="flex gap-4">
            <a
              href={GITHUB_REPO_URL}
              className="hover:text-indigo-600 dark:hover:text-indigo-400"
              target="_blank"
              rel="noopener noreferrer"
            >
              GitHub
            </a>
            <a
              href="#"
              className="hover:text-indigo-600 dark:hover:text-indigo-400"
            >
              Download Desktop
            </a>
          </div>
        </div>
      </footer>
    </div>
  );
}
