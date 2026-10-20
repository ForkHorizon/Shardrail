import { readFileSync } from "node:fs";

const locks = ["contracts", "client"].map((name) => JSON.parse(readFileSync(new URL(`../packages/${name}/package-lock.json`, import.meta.url))));
const allowed = new Set(["0BSD", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "MIT", "Python-2.0"]);
const failures = locks.flatMap((lock) => Object.entries(lock.packages)
  .filter(([path]) => path !== "")
  .flatMap(([path, metadata]) => {
    if (metadata.link || path.startsWith("../")) return [];
    const license = metadata.license;
    return typeof license === "string" && allowed.has(license)
      ? []
      : [`${path}: ${license ?? "license missing"}`];
  }));

if (failures.length) {
  console.error(`Unapproved or missing npm licenses:\n${failures.join("\n")}`);
  process.exitCode = 1;
} else {
  console.log("All locked npm packages have an approved license.");
}
