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

export function selectVariableStatements(source, names) {
  const wanted = new Set(names);
  const found = new Set();
  const statements = source.statements.filter((node) => {
    if (!ts.isVariableStatement(node)) return false;
    const selected = node.declarationList.declarations.filter((declaration) =>
      ts.isIdentifier(declaration.name) && wanted.has(declaration.name.text));
    for (const declaration of selected) found.add(declaration.name.text);
    return selected.length > 0;
  });
  for (const name of wanted) assert.ok(found.has(name), `${source.fileName}: ${name}`);
  return statements;
}

export function selectMountCallbacks(source, callNames) {
  const mounts = source.statements.filter((node) => ts.isExpressionStatement(node)
    && ts.isCallExpression(node.expression) && node.expression.expression.getText(source) === "onMount");
  return callNames.map((name) => {
    const matching = mounts.filter((mount) => {
      let found = false;
      function walk(node) {
        if (ts.isCallExpression(node) && node.expression.getText(source) === name) found = true;
        ts.forEachChild(node, walk);
      }
      walk(mount);
      return found;
    });
    assert.equal(matching.length, 1, `${source.fileName}: one onMount calling ${name}`);
    return matching[0].expression.arguments[0];
  });
}

export function compileTestScript(code, compilerOptions = {
  target: ts.ScriptTarget.ES2022,
  module: ts.ModuleKind.CommonJS,
}) {
  return ts.transpileModule(code, { compilerOptions }).outputText;
}
