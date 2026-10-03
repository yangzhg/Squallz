import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { readFile } from "node:fs/promises";
import ts from "typescript";

function parseSvelteScript(component, fileName) {
  const script = component.match(/<script lang="ts">([\s\S]*?)<\/script>/u)?.[1];
  assert.ok(script, `${fileName} has a TypeScript script`);
  return ts.createSourceFile(fileName, script, ts.ScriptTarget.Latest, true);
}

export function readSvelteScript(url, fileName) {
  return parseSvelteScript(readFileSync(url, "utf8"), fileName);
}

export async function readSvelteScriptAsync(url, fileName) {
  return parseSvelteScript(await readFile(url, "utf8"), fileName);
}

export function selectFunctions(source, names) {
  const wanted = new Set(names);
  const functions = source.statements.filter((node) =>
    ts.isFunctionDeclaration(node) && wanted.has(node.name?.text));
  for (const name of wanted) {
    assert.ok(functions.some((node) => node.name?.text === name), `${source.fileName}: ${name}`);
  }
  return functions;
}

export function compileTestScript(code, compilerOptions = {
  target: ts.ScriptTarget.ES2022,
  module: ts.ModuleKind.CommonJS,
}) {
  return ts.transpileModule(code, { compilerOptions }).outputText;
}
