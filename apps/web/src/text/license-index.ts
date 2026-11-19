export interface LicensedComponent {
  name: string;
  version: string;
  source: string;
  license: string;
  licensePath: string;
  products: readonly string[];
}
