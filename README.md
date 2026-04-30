# Complyx agent

## Instalación del agente

Antes de compilar el agente, es necesario instalar rust y cargo. [Consultar la documentación oficial](https://rust-lang.org/tools/install/)

Clona el repositorio

```bash
git clone https://github.com/styxiner/complyx-agent.git
cd complyx-agent
```

Instala la herramienta para hacer las migraciones de la base de datos:

```bash
cargo install sqlx-cli
```

Genera las migraciones para la base de datos:

```bash
sqlx database create --database-url sqlite:./complyx-agent.db
sqlx migrate run --database-url sqlite:./complyx-agent.db --source crates/local-db/migrations
DATABASE_URL="sqlite:./complyx-agent.db" cargo sqlx prepare --workspace
```

Compila el agente:

```bash
cargo build --release
```
