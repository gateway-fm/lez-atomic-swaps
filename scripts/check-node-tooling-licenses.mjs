import { readFileSync } from "node:fs";

const lock = JSON.parse(readFileSync("package-lock.json", "utf8"));
const allowed = new Set([
  "(CC-BY-4.0 AND OFL-1.1 AND MIT)",
  "(MPL-2.0 OR Apache-2.0)",
  "0BSD",
  "Apache-2.0",
  "BSD-3-Clause",
  "EPL-2.0",
  "ISC",
  "MIT",
  "Unlicense",
]);
const reviewedMissingMetadata = new Map([
  [
    "node_modules/khroma",
    {
      version: "2.1.0",
      integrity:
        "sha512-Ls993zuzfayK269Svk9hzpeGUKob/sIgZzyHYdjQoAdQetRKpOLj+k/QQQ/6Qi0Yz65mlROrfd+Ev+1+7dz9Kw==",
      // The exact upstream repository labels its lowercase `license` file MIT:
      // https://github.com/fabiospampinato/khroma/tree/master
      reviewedLicense: "MIT",
    },
  ],
]);

const rejected = [];
for (const [path, metadata] of Object.entries(lock.packages)) {
  if (path === "" || metadata.link) continue;
  const reviewed = reviewedMissingMetadata.get(path);
  if (
    !metadata.license &&
    reviewed?.version === metadata.version &&
    reviewed.integrity === metadata.integrity &&
    reviewed.reviewedLicense === "MIT"
  ) {
    continue;
  }
  if (!metadata.license || !allowed.has(metadata.license)) {
    rejected.push(`${path}: ${metadata.license ?? "missing"}`);
  }
}

if (rejected.length > 0) {
  throw new Error(`unreviewed Node tooling licenses:\n${rejected.join("\n")}`);
}

console.log("Node tooling licenses ok");
