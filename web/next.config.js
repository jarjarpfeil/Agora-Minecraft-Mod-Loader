/** @type {import('next').NextConfig} */
const nextConfig = {
  output: 'export',
  distDir: 'dist',
  images: {
    unoptimized: true,
  },
  experimental: {
    serverComponentsExternalPackages: ['sql.js'],
  },
  async headers() {
    return [
      {
        source: '/:path*',
        headers: [
          {
            key: 'Content-Security-Policy',
            value: [
              "default-src 'self';",
              "img-src 'self' data: https: blob:;",
              "style-src 'self' 'unsafe-inline';",
              "script-src 'self';",
              "connect-src 'self' https://api.github.com https://api.modrinth.com https://*.modrinthcdn.com https://cdn.modrinth.com;",
              "font-src 'self' data:;",
              "object-src 'none';",
              "base-uri 'self';",
              "frame-ancestors 'none';",
              "form-action 'self';",
            ].join(' '),
          },
        ],
      },
    ];
  },
};

export default nextConfig;
