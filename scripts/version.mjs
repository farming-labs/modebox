#!/usr/bin/env node
// One version for everything that gets published: the Cargo workspace (and
// its pinned modebox-core dependency), the npm packages, and the pins from
// `modebox` to its platform packages.
//
//   node scripts/version.mjs            print the current version
//   node scripts/version.mjs 0.2.0      set it everywhere (then run `cargo update -w`)
//   node scripts/version.mjs --check 0.2.0   fail unless everything is 0.2.0
import { readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const root = join(import.meta.dirname, '..');
const cargoToml = join(root, 'Cargo.toml');
const packages = readdirSync(join(root, 'packages')).map((d) => join(root, 'packages', d, 'package.json'));

const cargoVersion = (s) => s.match(/\[workspace\.package\][^[]*?\nversion = "([^"]+)"/)?.[1];
const corePin = (s) => s.match(/^modebox-core = \{[^}]*version = "([^"]+)"/m)?.[1];

function current() {
  const found = new Map();
  const cargo = readFileSync(cargoToml, 'utf8');
  found.set('Cargo.toml [workspace.package] version', cargoVersion(cargo));
  found.set('Cargo.toml modebox-core dependency', corePin(cargo));
  for (const p of packages) {
    const pkg = JSON.parse(readFileSync(p, 'utf8'));
    found.set(`${pkg.name} version`, pkg.version);
    for (const [dep, v] of Object.entries(pkg.optionalDependencies ?? {})) found.set(`${pkg.name} → ${dep}`, v);
  }
  return found;
}

function set(version) {
  let cargo = readFileSync(cargoToml, 'utf8');
  cargo = cargo.replace(/(\[workspace\.package\][^[]*?\nversion = ")[^"]+"/, `$1${version}"`);
  cargo = cargo.replace(/^(modebox-core = \{[^}]*version = ")[^"]+"/m, `$1${version}"`);
  writeFileSync(cargoToml, cargo);
  for (const p of packages) {
    const pkg = JSON.parse(readFileSync(p, 'utf8'));
    pkg.version = version;
    for (const dep of Object.keys(pkg.optionalDependencies ?? {})) pkg.optionalDependencies[dep] = version;
    writeFileSync(p, JSON.stringify(pkg, null, 2) + '\n');
  }
}

const args = process.argv.slice(2);
const check = args[0] === '--check';
const version = check ? args[1] : args[0];
if (version !== undefined && !/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(version)) {
  console.error(`not a semver version: ${version}`);
  process.exit(2);
}
if (!version) {
  const versions = new Set(current().values());
  console.log([...versions].join(', '));
} else if (check) {
  const wrong = [...current()].filter(([, v]) => v !== version);
  for (const [where, v] of wrong) console.error(`${where} is ${v}, expected ${version}`);
  if (wrong.length) process.exit(1);
  console.log(`everything is ${version}`);
} else {
  set(version);
  console.log(`set ${version}; now run \`cargo update -w\` to refresh Cargo.lock`);
}
