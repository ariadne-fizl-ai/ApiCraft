# syntax=docker/dockerfile:1
#
# The browser build of Fizl ApiCraft as a small static site: nginx running as
# a non-root user on port 8080, serving pre-compressed files.
#
#   docker build -t fizl-apicraft-web .
#   docker run --rm -p 8080:8080 fizl-apicraft-web
#
# Requests go straight from the browser (so CORS applies) unless a user runs
# apicraft-agent on their own machine with --allow-origin <this site's URL>.
# The agent isn't in the image: it's a local proxy that can start processes,
# not something to expose on a network.

ARG RUST_VERSION=1.95

# The wasm is the same for every platform, so this stage always runs on the
# builder's own architecture and only the nginx stage is per-platform.
FROM --platform=$BUILDPLATFORM rust:${RUST_VERSION}-slim-trixie AS build
ARG WASM_BINDGEN_VERSION=0.2.129
ARG BINARYEN_VERSION=133
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && rm -rf /var/lib/apt/lists/*
RUN rustup target add wasm32-unknown-unknown
# Prebuilt wasm-bindgen (must match the crate's pinned version) and wasm-opt.
RUN set -eu; \
    arch="$(uname -m)"; \
    case "$arch" in x86_64|aarch64) ;; *) echo "unsupported build architecture: $arch" >&2; exit 1 ;; esac; \
    bg="wasm-bindgen-${WASM_BINDGEN_VERSION}-${arch}-unknown-linux-musl"; \
    curl -fsSL "https://github.com/wasm-bindgen/wasm-bindgen/releases/download/${WASM_BINDGEN_VERSION}/${bg}.tar.gz" | tar -xz -C /usr/local/bin --strip-components=1 "${bg}/wasm-bindgen"; \
    curl -fsSL "https://github.com/WebAssembly/binaryen/releases/download/version_${BINARYEN_VERSION}/binaryen-version_${BINARYEN_VERSION}-${arch}-linux.tar.gz" | tar -xz -C /opt; \
    ln -s "/opt/binaryen-version_${BINARYEN_VERSION}/bin/wasm-opt" /usr/local/bin/wasm-opt; \
    wasm-bindgen --version; wasm-opt --version

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY apps apps
COPY xtask xtask
COPY web web
# The same steps as `cargo xtask web --release`.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p apicraft-web --lib --target wasm32-unknown-unknown \
 && wasm-bindgen --target web --no-typescript --out-dir /site/pkg target/wasm32-unknown-unknown/release/apicraft_web.wasm \
 && wasm-opt -Oz --enable-bulk-memory --enable-nontrapping-float-to-int -o /site/pkg/apicraft_web_bg.wasm /site/pkg/apicraft_web_bg.wasm
# nginx serves the .gz copies to browsers that accept them (gzip_static).
RUN cp -r web/. /site/ \
 && find /site -type f \( -name '*.wasm' -o -name '*.js' -o -name '*.css' -o -name '*.html' -o -name '*.svg' \) -exec gzip -9 -k -n {} + \
 && ls -l /site/pkg

# alpine-slim: nginx's core modules (gzip_static included) without njs and friends.
FROM nginxinc/nginx-unprivileged:1.30-alpine-slim
COPY apps/web/docker/nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=build /site /usr/share/nginx/html
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=3s CMD wget -q -O /dev/null http://127.0.0.1:8080/ || exit 1
