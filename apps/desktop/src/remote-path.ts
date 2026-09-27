export function remoteParentPath(path: string): string {
  if (path === "." || path === "/") return path;
  const withoutTrailingSlashes = path.replace(/\/+$/, "") || "/";
  const separator = withoutTrailingSlashes.lastIndexOf("/");
  return separator < 0 ? "." : separator === 0 ? "/" : withoutTrailingSlashes.slice(0, separator);
}

export function remoteChildPath(parent: string, name: string): string {
  if (parent === ".") return `./${name}`;
  if (parent === "/") return `/${name}`;
  return `${parent.replace(/\/+$/, "")}/${name}`;
}
