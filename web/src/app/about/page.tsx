export const metadata = {
  title: 'About — Agora',
};

export default function AboutPage() {
  return (
    <div className="mx-auto max-w-3xl space-y-8">
      <div>
        <h1 className="text-3xl font-bold">About Agora</h1>
        <p className="mt-2 text-gray-600 dark:text-gray-400">
          A different kind of Minecraft mod platform.
        </p>
      </div>

      <section className="space-y-4">
        <h2 className="text-xl font-semibold">The mission</h2>
        <p>
          Agora is a decentralized, ad-free, open-source Minecraft mod launcher and discovery platform. The goal is to return control to the community instead of locking it inside corporate infrastructure.
        </p>
        <ul className="list-disc space-y-2 pl-6">
          <li>
            <strong>$0/month server footprint.</strong> Data ships through GitHub Release Assets and static sites.
          </li>
          <li>
            <strong>Security by delegation.</strong> The app does not handle Microsoft/Xbox auth or run the JVM; it delegates all of that to the official Mojang launcher.
          </li>
          <li>
            <strong>Curated, not warehoused.</strong> Every entry is community reviewed and voted on.
          </li>
        </ul>
      </section>

      <section className="space-y-4">
        <h2 className="text-xl font-semibold">How it works</h2>
        <p>
          Mods, packs, shaders, and other assets are stored as flat JSON files in this repository. A nightly compiler reads those files, resolves release metadata, and builds a signed SQLite database called <code>registry.db</code>.
        </p>
        <p>
          The desktop launcher downloads that database, verifies its signature, and uses it to browse, install, and launch curated content. The website renders the same catalog as a public, search-engine-friendly directory.
        </p>
      </section>

      <section className="space-y-4">
        <h2 className="text-xl font-semibold">Open source</h2>
        <p>
          All curation, governance, and code is public. Reviews and votes happen through structured GitHub interactions, and the audit log is transparent by design.
        </p>
      </section>
    </div>
  );
}
