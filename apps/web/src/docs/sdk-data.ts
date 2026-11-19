export type FocalApiDeclaration = {
  kind: 'struct' | 'enum' | 'function' | 'macro' | 'typedef' | 'variable';
  name: string;
  signature: string;
  comment: string;
  members: { name: string; type: string; comment: string }[];
};
export type FocalPreview = {
  title: string;
  slug: string;
  description: string;
  source: string;
  line: number;
  width: number;
  height: number;
  page: string;
};
export type FocalComponent = {
  title: string;
  slug: string;
  header: string;
  summary: string;
  declarations: FocalApiDeclaration[];
  previews: FocalPreview[];
};
export interface FocalSdkDocs {
  schema: 1;
  components: readonly FocalComponent[];
}

declare const __FUJIMOD_FOCAL_COMPONENTS__: readonly FocalComponent[];
export const focalComponents = __FUJIMOD_FOCAL_COMPONENTS__;
