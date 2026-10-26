// Describe the registered JavaScript module to Python's checker using the
// binding's own TypeScript declarations. Generated files stay under target/.
import ts from 'typescript-api';
import { mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';

const source = resolve('bindings/javascript/src/index.ts');
const program = ts.createProgram([source], {
  target: ts.ScriptTarget.ES2022,
  module: ts.ModuleKind.NodeNext,
  moduleResolution: ts.ModuleResolutionKind.NodeNext,
  strict: true,
  allowJs: false,
  skipLibCheck: true,
});
const diagnostics = ts.getPreEmitDiagnostics(program);
if (diagnostics.length) {
  throw new Error(
    ts.formatDiagnosticsWithColorAndContext(diagnostics, {
      getCanonicalFileName: (file) => file,
      getCurrentDirectory: () => process.cwd(),
      getNewLine: () => '\n',
    }),
  );
}
const checker = program.getTypeChecker();
const file = program.getSourceFile(source);
if (!file) throw Error(`Missing source: ${source}`);
const module = checker.getSymbolAtLocation(file);
if (!module) throw Error(`Missing module: ${source}`);
const exports = checker.getExportsOfModule(module);
const names = new Map<ts.Type, string>();
const used = new Set<string>();
const queue: [ts.Type, string][] = [];
const definitions: string[] = [];
const keyword = new Set([
  'from',
  'class',
  'None',
  'True',
  'False',
  'in',
  'is',
  'global',
  'pass',
  'def',
  'return',
  'lambda',
  'with',
  'as',
  'async',
  'await',
  'type',
]);
const identifier = (name: string): string => (keyword.has(name) ? `${name}_` : name);

function nameFor(type: ts.Type, hint: string): string {
  const known = names.get(type);
  if (known) return known;
  const symbol = type.aliasSymbol ?? type.getSymbol();
  const original = symbol?.getName();
  const constructor =
    Boolean(symbol && symbol.flags & ts.SymbolFlags.Class) &&
    !(type.isClassOrInterface() && type.objectFlags & ts.ObjectFlags.Class);
  const base =
    `Js${original && original !== 'Readonly' && !original.startsWith('__') ? original : hint}${constructor ? 'Constructor' : ''}`.replace(
      /[^a-zA-Z0-9_]/g,
      '_',
    );
  let name = base;
  for (let suffix = 2; used.has(name); suffix++) name = `${base}${suffix}`;
  used.add(name);
  names.set(type, name);
  queue.push([type, name]);
  return name;
}

function python(type: ts.Type, hint: string): string {
  const flags = type.flags;
  if (flags & (ts.TypeFlags.Any | ts.TypeFlags.Unknown)) return 'object';
  if (flags & ts.TypeFlags.NonPrimitive) return 'object';
  if (flags & ts.TypeFlags.Never) return 'Never';
  if (flags & (ts.TypeFlags.Null | ts.TypeFlags.Undefined | ts.TypeFlags.Void)) return 'None';
  if (flags & ts.TypeFlags.StringLiteral)
    return `Literal[${JSON.stringify((type as ts.LiteralType).value)}]`;
  if (flags & ts.TypeFlags.NumberLiteral) return `Literal[${(type as ts.NumberLiteralType).value}]`;
  if (flags & ts.TypeFlags.BooleanLiteral)
    return `Literal[${(type as ts.Type & { intrinsicName: string }).intrinsicName === 'true' ? 'True' : 'False'}]`;
  if (flags & ts.TypeFlags.StringLike) return 'str';
  if (flags & (ts.TypeFlags.NumberLike | ts.TypeFlags.BigIntLike)) return 'int';
  if (flags & ts.TypeFlags.Boolean) return 'bool';
  if (flags & ts.TypeFlags.EnumLike) return 'int';
  if (type.isUnion()) {
    if (type.aliasSymbol) return nameFor(type, hint);
    return [...new Set(type.types.map((part) => python(part, hint)))].join(' | ');
  }
  const symbol = type.getSymbol()?.getName();
  const args = checker.getTypeArguments(type as ts.TypeReference);
  if (symbol === 'Promise') return `Awaitable[${python(args[0], `${hint}Result`)}]`;
  if (symbol === 'Uint8Array' || symbol === 'ArrayBuffer') return 'JsBytes';
  if (symbol && /^(?:Uint|Int|Float)\d+(?:Clamped)?Array$/.test(symbol)) return 'JsNumbers';
  if (checker.isTupleType(type)) return `tuple[${args.map((arg) => python(arg, hint)).join(', ')}]`;
  if (checker.isArrayType(type) || symbol === 'ReadonlyArray')
    return `Sequence[${python(args[0], `${hint}Item`)}]`;
  if (flags & ts.TypeFlags.Object) return nameFor(type, hint);
  throw Error(`Unsupported TypeScript type ${checker.typeToString(type)} in ${hint}`);
}

function signature(method: string, signature: ts.Signature, hint: string): string {
  const args = signature.getParameters().map((symbol, index) => {
    const declaration = symbol.valueDeclaration ?? symbol.declarations?.[0];
    const type = checker.getTypeOfSymbolAtLocation(symbol, declaration ?? file!);
    const optional =
      Boolean(symbol.flags & ts.SymbolFlags.Optional) ||
      (declaration && ts.isParameter(declaration) && declaration.questionToken) ||
      (declaration && ts.isParameter(declaration) && declaration.initializer);
    const rest =
      declaration && ts.isParameter(declaration) && declaration.dotDotDotToken ? '*' : '';
    const name = identifier(symbol.getName());
    if (rest && checker.isTupleType(type)) {
      return `*${name}: *${python(type, `${hint}Arguments`)}`;
    }
    return `${rest}${name}: ${python(type, `${hint}Argument${index}`)}${optional ? ' = ...' : ''}`;
  });
  const result = python(checker.getReturnTypeOfSignature(signature), `${hint}Result`);
  return `    def ${method}(self${args.length ? ', ' + args.join(', ') : ''}) -> ${result}: ...`;
}

function methods(method: string, signatures: readonly ts.Signature[], hint: string): string[] {
  return signatures.flatMap((item) => [
    ...(signatures.length > 1 ? ['    @overload'] : []),
    signature(method, item, hint),
  ]);
}

const registrations = exports
  .filter((symbol) => ['usb', 'catalog', 'valueSchemas'].includes(symbol.getName()))
  .map((symbol) => {
    const type = checker.getTypeOfSymbolAtLocation(symbol, symbol.valueDeclaration ?? file);
    return `${symbol.getName()}: ${python(type, symbol.getName())}`;
  });
for (const symbol of exports.filter((item) =>
  ['UsbError', 'DefinitionError', 'ServiceCommandError'].includes(item.getName()),
)) {
  python(checker.getDeclaredTypeOfSymbol(symbol), symbol.getName());
}

while (queue.length) {
  const entry = queue.shift();
  if (!entry) break;
  const [type, name] = entry;
  if (type.isUnion()) {
    definitions.push(
      `${name} = ${[...new Set(type.types.map((part) => python(part, name)))].join(' | ')}\n`,
    );
    continue;
  }
  const lines: string[] = [];
  lines.push(
    ...methods('__call__', checker.getSignaturesOfType(type, ts.SignatureKind.Call), name),
  );
  lines.push(
    ...methods('new', checker.getSignaturesOfType(type, ts.SignatureKind.Construct), name),
  );
  for (const property of checker.getPropertiesOfType(type)) {
    const field = property.getName();
    if (field.startsWith('__') || field.startsWith('#')) continue;
    const declaration = property.valueDeclaration ?? property.declarations?.[0];
    if (
      declaration &&
      ts.canHaveModifiers(declaration) &&
      ts
        .getModifiers(declaration)
        ?.some((modifier) => modifier.kind === ts.SyntaxKind.PrivateKeyword)
    )
      continue;
    const value = checker.getTypeOfSymbolAtLocation(property, declaration ?? file);
    const callable = checker.getSignaturesOfType(value, ts.SignatureKind.Call);
    if (callable.length) lines.push(...methods(identifier(field), callable, name + field));
    else lines.push(`    ${identifier(field)}: ${python(value, name + field)}`);
  }
  definitions.push(`class ${name}(Protocol):\n${lines.length ? lines.join('\n') : '    pass'}\n`);
}

const header =
  '# Generated from the JavaScript binding types.\nfrom collections.abc import Awaitable, Buffer, Iterator, Sequence\nfrom typing import Literal, Never, Protocol, overload\n\nclass JsBytes(Protocol):\n    def __getitem__(self, index: int) -> int: ...\n    def to_py(self) -> Buffer: ...\n\nclass JsNumbers(Protocol):\n    def __iter__(self) -> Iterator[int]: ...\n    def to_py(self) -> list[int]: ...\n\n';
mkdirSync('target/python-types', { recursive: true });
writeFileSync(
  'target/python-types/_fujimod_js.pyi',
  header + definitions.join('\n') + '\n' + registrations.join('\n') + '\n',
);
