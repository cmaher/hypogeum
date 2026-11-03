// Global setup for auth.spec.mjs: an isolated copy of the local stack
// (PostgreSQL, Kratos, Caddy and the app) on free ports, so it never touches
// the development stack or anything else bound to 8080/3000/4433/5432.
//
// - A separate Compose project (`exemplum-browser-smoke`) with its own volume.
// - Host ports come from EXEMPLUM_POSTGRES_PORT, EXEMPLUM_KRATOS_PORT,
//   EXEMPLUM_CADDY_PORT and EXEMPLUM_CADDY_APP_PORT.
// - kratos.yml hardcodes the browser origin http://localhost:8080, so a scratch
//   copy with that origin rewritten is mounted via EXEMPLUM_KRATOS_CONFIG_DIR.
// - Default development secrets; env/local.env is ignored.
//
// The returned function tears everything down, including the volume.
import { execFileSync, spawn } from "node:child_process";
import { copyFileSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const EXAMPLE = resolve(import.meta.dirname, "../..");
const PROJECT = "exemplum-browser-smoke";
const ORIGIN = "http://localhost:8080";

function freePort() {
  return new Promise((done, fail) => {
    const server = createServer();
    server.on("error", fail);
    server.listen(0, "127.0.0.1", () => {
      const { port } = server.address();
      server.close(() => done(String(port)));
    });
  });
}

async function waitFor(url, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      if ((await fetch(url)).ok) return;
    } catch {
      // not listening yet
    }
    await new Promise((done) => setTimeout(done, 250));
  }
  throw new Error(`${url} did not become ready`);
}

export default async function globalSetup() {
  const [postgres, kratos, caddy, app] = [
    await freePort(),
    await freePort(),
    await freePort(),
    await freePort(),
  ];
  const origin = `http://localhost:${caddy}`;
  const config = mkdtempSync(join(tmpdir(), "exemplum-kratos-"));
  const source = join(EXAMPLE, "infra/kratos");
  writeFileSync(
    join(config, "kratos.yml"),
    readFileSync(join(source, "kratos.yml"), "utf8").replaceAll(ORIGIN, origin),
  );
  copyFileSync(join(source, "identity.schema.json"), join(config, "identity.schema.json"));

  const db = (user) => `postgres://${user}:password@127.0.0.1:${postgres}/exemplum`;
  const env = {
    ...process.env,
    EXEMPLUM_ENV_FILE: "",
    // Pin the development secrets the URLs below assume, overriding any exported values.
    EXEMPLUM_POSTGRES_PASSWORD: "password",
    EXEMPLUM_MIGRATION_PASSWORD: "password",
    EXEMPLUM_USERS_PASSWORD: "password",
    EXEMPLUM_TASKS_PASSWORD: "password",
    EXEMPLUM_KRATOS_DB_PASSWORD: "password",
    EXEMPLUM_KRATOS_COOKIE_SECRET: "exemplum-local-cookie-secret-0123456789",
    EXEMPLUM_KRATOS_CIPHER_SECRET: "local-cipher-secret-0123456789ab",
    EXEMPLUM_POSTGRES_PORT: postgres,
    EXEMPLUM_KRATOS_PORT: kratos,
    EXEMPLUM_CADDY_PORT: caddy,
    EXEMPLUM_CADDY_APP_PORT: app,
    EXEMPLUM_KRATOS_CONFIG_DIR: config,
    EXEMPLUM_MIGRATION_DATABASE_URL: db("exemplum_migrator"),
    EXEMPLUM_ADMIN_DATABASE_URL: db("exemplum_users"),
    EXEMPLUM_APP_DATABASE_URL: db("exemplum_users"),
    EXEMPLUM_APP_BIND: `127.0.0.1:${app}`,
    EXEMPLUM_APP_KRATOS_PUBLIC_URL: `http://127.0.0.1:${kratos}`,
    EXEMPLUM_APP_KRATOS_BROWSER_URL: `${origin}/.ory/kratos`,
    EXEMPLUM_LOG: "warn",
  };
  // Output is shown only when a step fails.
  const run = (command, args) => {
    try {
      execFileSync(command, args, { cwd: EXAMPLE, env, stdio: "pipe" });
    } catch (error) {
      process.stderr.write(`${error.stdout ?? ""}${error.stderr ?? ""}`);
      throw error;
    }
  };
  const compose = (...args) => run("docker", ["compose", "-p", PROJECT, ...args]);

  let server;
  const teardown = async () => {
    server?.kill("SIGTERM");
    compose("down", "--volumes", "--remove-orphans");
    rmSync(config, { recursive: true, force: true });
  };
  try {
    compose("down", "--volumes", "--remove-orphans");
    compose("up", "--detach", "--wait", "postgres");
    compose(
      "exec", "-T", "postgres", "psql", "-X", "-q", "-U", "admin", "-d", "exemplum",
      "-f", "/provision/provision.sql",
    );
    run("cargo", ["build", "--locked", "-p", "exemplum-db-cli", "-p", "exemplum-app-cli", "-p", "exemplum-admin-cli"]);
    run("cargo", ["run", "--locked", "-q", "-p", "exemplum-db-cli", "--bin", "exemplum-db", "--", "migrate"]);
    compose("run", "--rm", "kratos-migrate");
    compose("up", "--detach", "--wait", "kratos", "caddy");
    server = spawn("cargo", ["run", "--locked", "-q", "-p", "exemplum-app-cli", "--bin", "app"], {
      cwd: EXAMPLE,
      env,
      stdio: "inherit",
    });
    await waitFor(`http://127.0.0.1:${app}/healthz`, 120_000);
    await waitFor(`${origin}/healthz`, 30_000);
  } catch (error) {
    await teardown();
    throw error;
  }
  // Inherited by the test workers.
  process.env.EXEMPLUM_SMOKE_BASE_URL = origin;
  process.env.EXEMPLUM_SMOKE_ADMIN_DATABASE_URL = env.EXEMPLUM_ADMIN_DATABASE_URL;
  return teardown;
}
