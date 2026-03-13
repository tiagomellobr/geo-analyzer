# ── Estágio 1: build ─────────────────────────────────────────────────────────
FROM rust:1.88-slim AS builder

# Dependências de sistema necessárias para sqlx (OpenSSL via rustls não precisa,
# mas o linker precisa de build-essential e pkg-config para algumas crates)
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copiar manifesto primeiro (cache de dependências)
COPY Cargo.toml Cargo.lock ./

# Criar src/main.rs dummy para compilar dependências em cache
RUN mkdir -p src && echo "fn main() {}" > src/main.rs

# Baixar e compilar apenas as dependências (camada cacheável)
RUN cargo build --release 2>&1 || true

# Remover o artefato dummy para recompilar com o código real
RUN rm -f target/release/deps/geo_analyzer* src/main.rs

# Copiar código fonte real
COPY src ./src
COPY templates ./templates
COPY migrations ./migrations

# Build final
RUN cargo build --release

# ── Estágio 2: runtime ────────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copiar binário compilado
COPY --from=builder /app/target/release/geo-analyzer /app/geo-analyzer

# Criar diretório para o banco SQLite
RUN mkdir -p /data

EXPOSE 3000

ENV DATABASE_URL="sqlite:///data/geo_analyzer.db"
ENV BIND="0.0.0.0:3000"
ENV RUST_LOG="geo_analyzer=info,tower_http=info"

ENTRYPOINT ["/app/geo-analyzer"]
