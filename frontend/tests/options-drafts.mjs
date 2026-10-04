import assert from "node:assert/strict";
import vm from "node:vm";
import ts from "typescript";
import { compileTestScript, selectFunctions } from "./source.mjs";

function installOptions(source, context, name, setup = "") {
  const variables = source.statements.filter(ts.isVariableStatement)
    .flatMap((node) => Array.from(node.declarationList.declarations));
  const declaration = variables.find((node) => ts.isIdentifier(node.name) && node.name.text === name);
  assert.ok(declaration, `App constructs ${name}`);
  const aliases = variables.filter((node) => ts.isIdentifier(node.name)
    && node.initializer && ts.isCallExpression(node.initializer)
    && ts.isIdentifier(node.initializer.expression) && node.initializer.expression.text === "$derived"
    && node.initializer.arguments[0]?.getText(source).startsWith(`${name}.`));
  assert.ok(aliases.length > 0, `App reads ${name} through derived values`);
  const readers = aliases.map((node) => `${node.name.text}: () => (${node.initializer.arguments[0].getText(source)})`);
  const installed = vm.runInNewContext(compileTestScript(`${setup}
    const ${declaration.getText(source)};
    ({options: ${name}, readers: {${readers.join(",")}}})`), context);
  context[name] = installed.options;
  for (const [name, get] of Object.entries(installed.readers)) {
    Object.defineProperty(context, name, { get });
  }
  return installed.options;
}

export function installCreateOptions(source, context) {
  const formatCheck = selectFunctions(source, ["isCreateFormatId"])[0];
  return installOptions(source, context, "createOptions", formatCheck.getText(source));
}

export function installExtractOptions(source, context) {
  return installOptions(source, context, "extractOptions");
}

export function installBatchExtractDraft(source, context) {
  return installOptions(source, context, "batchExtract");
}
