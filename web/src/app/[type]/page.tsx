import { notFound } from 'next/navigation';
import { getAllItems, contentTypeFromPath, contentTypeLabel, contentTypePath } from '@/lib/db';
import { Catalog } from '@/components/Catalog';

export function generateStaticParams() {
  return ['mod', 'pack', 'shader', 'resourcepack', 'server', 'datapack', 'world'].map((type) => ({
    type: `${type}s`,
  }));
}

interface TypePageProps {
  params: { type: string };
}

export default async function TypePage({ params }: TypePageProps) {
  const contentType = contentTypeFromPath(params.type);
  if (!contentType) {
    notFound();
  }

  const items = await getAllItems(contentType);

  return (
    <Catalog
      items={items}
      typeLabel={contentTypeLabel(contentType)}
      typePath={contentTypePath(contentType)}
    />
  );
}
