// A fixture for PG-E0-15: the manifest declares a dependency the sources never
// import, so the frontend dead-dependency gate must fail here.
export function answer(): number {
  return 42;
}
