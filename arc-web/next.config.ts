import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  output: "standalone",
  // Next.js 16 uses Turbopack by default. Turbopack has native WebAssembly
  // support (module type `wasm`) and does not need `asyncWebAssembly` in
  // webpack experiments. Declaring an empty `turbopack` config here:
  //   (a) silences the "webpack config present but no turbopack config" error,
  //   (b) opts us into Turbopack's built-in .wasm handling for C4.3+ imports.
  // If a future CI step forces webpack mode (`next build --webpack`), the
  // webpack function below re-enables asyncWebAssembly on the client bundle.
  // Source: node_modules/next/dist/docs/01-app/03-api-reference/05-config/01-next-config-js/turbopack.md
  turbopack: {},
  webpack(config, { isServer }) {
    // Guard: only needed when Turbopack is not active (e.g. --webpack flag).
    // Enables async WebAssembly for the beacon-blocks-wasm bundler-target
    // package on the client bundle only.
    if (!isServer) {
      config.experiments = {
        ...config.experiments,
        asyncWebAssembly: true,
      };
    }
    return config;
  },
};

export default nextConfig;
