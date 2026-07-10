export const GITHUB_REPO_URL =
  process.env.NEXT_PUBLIC_GITHUB_REPO_URL || 'https://github.com/jarjarpfeil/Agora-Minecraft-Mod-Loader';

export const GITHUB_RELEASES_URL = `${GITHUB_REPO_URL}/releases`;

// Registry snapshots are published as non-prerelease `registry-*` releases, so
// GitHub's `/releases/latest` endpoint points to a database asset rather than
// the desktop installer. Consumers should fetch this list and select a `v*`
// desktop release instead.
export const GITHUB_API_RELEASES_URL =
  `https://api.github.com/repos/${GITHUB_REPO_URL.replace('https://github.com/', '')}/releases?per_page=100`;
