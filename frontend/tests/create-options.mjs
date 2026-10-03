import assert from "node:assert/strict";
import vm from "node:vm";
import ts from "typescript";
import { compileTestScript, selectFunctions } from "./source.mjs";

export function installCreateOptions(source, context) {
  const variables = source.statements.filter(ts.isVariableStatement)
    .flatMap((node) => Array.from(node.declarationList.declarations));
  const declaration = variables.find((node) => ts.isIdentifier(node.name) && node.name.text === "createOptions");
  assert.ok(declaration, "App constructs its editable create options");
  const aliases = variables.filter((node) => ts.isIdentifier(node.name)
    && node.initializer && ts.isCallExpression(node.initializer)
    && ts.isIdentifier(node.initializer.expression) && node.initializer.expression.text === "$derived"
    && node.initializer.arguments[0]?.getText(source).startsWith("createOptions."));
  assert.ok(aliases.length > 0, "App reads its create options through derived values");
  const readers = aliases.map((node) => `${node.name.text}: () => (${node.initializer.arguments[0].getText(source)})`);
  const formatCheck = selectFunctions(source, ["isCreateFormatId"])[0];
  const installed = vm.runInNewContext(compileTestScript(`${formatCheck.getText(source)}
    const ${declaration.getText(source)};
    ({createOptions, readers: {${readers.join(",")}}})`), context);
  context.createOptions = installed.createOptions;
  for (const [name, get] of Object.entries(installed.readers)) {
    Object.defineProperty(context, name, { get });
  }
  return installed.createOptions;
}
