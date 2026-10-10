/**
 * A node of a kind Viva Studio does not model yet. It stays visible rather
 * than silently absent; a declaration the library could not build at all is
 * listed under Diagnostics instead (backlog DX-05).
 */
export function UnknownNodeView({ tag }: { tag: string }) {
  return (
    <p className="unknown-node">
      Viva Studio cannot show or edit a <code>{tag}</code> node.
    </p>
  );
}
