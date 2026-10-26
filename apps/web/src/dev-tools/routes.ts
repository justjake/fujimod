/** Dev Tools' guided tools, each opened at `/dev-tools/<tool>/`. */
export const Wizard = {
  Disable: 'disable',
  Service: 'service',
  StateCaptures: 'state-captures',
  MapControls: 'map-controls',
} as const;
export type Wizard = (typeof Wizard)[keyof typeof Wizard];

export const toolPath = (tool?: Wizard) => (tool ? `/dev-tools/${tool}/` : '/dev-tools/');

/** The tool a Dev Tools path opens; undefined for the workbench or any other path. */
export function toolAt(pathname: string): Wizard | undefined {
  const name = /^\/dev-tools\/([^/]+)\/?$/.exec(pathname)?.[1];
  return Object.values(Wizard).find((tool) => tool === name);
}

export const isDevToolsPath = (pathname: string) =>
  /^\/dev-tools\/?$/.test(pathname) || toolAt(pathname) !== undefined;
