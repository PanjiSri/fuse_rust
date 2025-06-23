FROM rust:latest

WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
    libfuse3-dev \
    pkg-config \
    fuse3 \
    sqlite3 \
    sudo \
    procps \
    diffutils \
    && rm -rf /var/lib/apt/lists/*

COPY . .

CMD ["bash"]