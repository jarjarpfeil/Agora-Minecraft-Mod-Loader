import Link from 'next/link';
import { notFound } from 'next/navigation';
import { getItemById, getItemIds, isContentType, contentTypeLabel, contentTypePath, CONTENT_TYPES } from '@/lib/db';

interface DetailPageProps {
  params: { type: string; id: string };
}

export async function generateStaticParams() {
  const params: { type: string; id: string }[] = [];
  for (const type of CONTENT_TYPES) {
    const ids = await getItemIds(type);
    params.push(...ids.map((id) => ({ type, id })));
  }
  return params;
}

export default async function DetailPage({ params }: DetailPageProps) {
  if (!isContentType(params.type)) {
    notFound();
  }

  const item = await getItemById(params.id);
  if (!item || item.content_type !== params.type) {
    notFound();
  }

  return (
    <div className="space-y-8">
      <div>
        <Link
          href={contentTypePath(params.type)}
          className="text-sm text-indigo-600 hover:underline dark:text-indigo-400"
        >
          ← Back to {contentTypeLabel(params.type)}
        </Link>
        <h1 className="mt-2 text-3xl font-bold">{item.name}</h1>
        <p className="text-gray-600 dark:text-gray-400">
          {contentTypeLabel(params.type)} · {item.download_strategy}
        </p>
      </div>

      {item.icon_url && (
        <img
          src={item.icon_url}
          alt={`${item.name} icon`}
          className="h-24 w-24 rounded-xl border object-contain dark:border-gray-700"
        />
      )}

      <div className="rounded-xl border bg-white p-6 dark:border-gray-700 dark:bg-gray-800">
        <h2 className="mb-2 text-xl font-semibold">Curator note</h2>
        <div className="prose prose-sm max-w-none whitespace-pre-line text-gray-700 dark:prose-invert dark:text-gray-300">
          {item.curator_note}
        </div>
      </div>

      {item.categories.length > 0 && (
        <div>
          <h2 className="mb-2 text-lg font-semibold">Categories</h2>
          <div className="flex flex-wrap gap-2">
            {item.categories.map((cat) => (
              <span
                key={cat}
                className="rounded-md bg-gray-100 px-3 py-1 text-sm text-gray-700 dark:bg-gray-700 dark:text-gray-300"
              >
                {cat}
              </span>
            ))}
          </div>
        </div>
      )}

      {item.compatible_versions.length > 0 && (
        <div>
          <h2 className="mb-2 text-lg font-semibold">Compatible versions</h2>
          <ul className="list-disc space-y-1 pl-6">
            {item.compatible_versions.map((v, i) => (
              <li key={i} className="text-gray-700 dark:text-gray-300">
                {v.mc_version} · {v.loader} · {v.mod_version}
              </li>
            ))}
          </ul>
        </div>
      )}

      <div className="flex flex-wrap gap-6 text-sm">
        <div>
          <span className="font-semibold">Net score:</span>{' '}
          <span className="text-green-700 dark:text-green-400">{item.net_score}</span>
        </div>
        <div>
          <span className="font-semibold">Source:</span>{' '}
          <code className="rounded bg-gray-100 px-1 dark:bg-gray-800">{item.source_identifier}</code>
        </div>
        <div>
          <span className="font-semibold">Strategy:</span> {item.download_strategy}
        </div>
      </div>
    </div>
  );
}
