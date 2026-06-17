import { notFound } from 'next/navigation';
import { getAllItems, isContentType, contentTypeLabel, contentTypePath } from '@/lib/db';
import { Catalog } from '@/components/Catalog';

export function generateStaticParams() {
  return ['mod', 'pack', 'shader', 'resourcepack', 'server', 'datapack', 'world'].map((type) => ({
    type,
  }));
}

interface TypePageProps {
  params: { type: string };
}

export default async function TypePage({ params }: TypePageProps) {
  if (!isContentType(params.type)) {
    notFound();
  }

  const items = await getAllItems(params.type);

  return (
    <Catalog
      items={items}
      typeLabel={contentTypeLabel(params.type)}
      typePath={contentTypePath(params.type)}
    />
  );
}
