import {
  Button,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  TableSelectAll,
  TableSelectRow,
  Tag,
} from '@carbon/react';
import { formatBytes, formatPercent } from '../format';
import type { ViewNode } from '../ipc';
import { OTHER_LABEL } from '../treemap/layout';
import { joinPath } from '../treemap/paths';
import { toSelectedItem, type SelectedItem } from '../treemap/selection';

interface FolderTableProps {
  view: ViewNode;
  path: string;
  selectedIds: ReadonlySet<number>;
  onToggle: (item: SelectedItem) => void;
  onToggleAll: (items: SelectedItem[], on: boolean) => void;
  onOpen: (id: number) => void;
}

const KIND_LABEL: Record<ViewNode['kind'], string> = {
  dir: 'Folder',
  file: 'File',
  inaccessible: 'Not accessible',
  other: 'Aggregate',
};

/**
 * The treemap's data as a table. This is the keyboard and screen reader path:
 * everything the canvas offers (select, open, sizes) is reachable here.
 */
export function FolderTable({
  view,
  path,
  selectedIds,
  onToggle,
  onToggleAll,
  onOpen,
}: FolderTableProps) {
  const rows = (view.children ?? []).map((node) => ({
    node,
    item: toSelectedItem({ ...node, path: joinPath(path, node.name) }),
  }));
  const selectable = rows.flatMap((row) => (row.item ? [row.item] : []));
  const selectedCount = selectable.filter((item) => selectedIds.has(item.id)).length;
  const allSelected = selectable.length > 0 && selectedCount === selectable.length;

  if (rows.length === 0) {
    return <p className="folder-table__empty">This folder is empty.</p>;
  }

  return (
    <Table size="sm" aria-label={`Contents of ${path}, largest first`}>
      <TableHead>
        <TableRow>
          <TableSelectAll
            id="folder-select-all"
            name="folder-select-all"
            aria-label="Select all items in this folder"
            checked={allSelected}
            indeterminate={selectedCount > 0 && !allSelected}
            disabled={selectable.length === 0}
            onSelect={() => onToggleAll(selectable, !allSelected)}
          />
          <TableHeader>Name</TableHeader>
          <TableHeader>Type</TableHeader>
          <TableHeader className="folder-table__number">Size</TableHeader>
          <TableHeader className="folder-table__number">Share</TableHeader>
          <TableHeader className="folder-table__number">Files</TableHeader>
        </TableRow>
      </TableHead>
      <TableBody>
        {rows.map(({ node, item }, index) => {
          const label = node.kind === 'other' ? OTHER_LABEL : node.name;
          const id = `folder-row-${index}`;
          return (
            <TableRow
              key={node.id ?? 'other'}
              isSelected={!!item && selectedIds.has(item.id)}
            >
              {item ? (
                <TableSelectRow
                  id={id}
                  name={id}
                  aria-label={`Select ${label}`}
                  checked={selectedIds.has(item.id)}
                  onSelect={() => onToggle(item)}
                />
              ) : (
                <TableCell />
              )}
              <TableCell>
                {node.kind === 'dir' && node.id !== undefined ? (
                  <Button
                    kind="ghost"
                    size="sm"
                    className="folder-table__open"
                    onClick={() => onOpen(node.id as number)}
                    aria-label={`Open folder ${label}`}
                  >
                    {label}
                  </Button>
                ) : (
                  <span className="folder-table__name">{label}</span>
                )}
              </TableCell>
              <TableCell>
                {node.kind === 'inaccessible' ? (
                  <Tag type="red" size="sm">
                    {KIND_LABEL[node.kind]}
                  </Tag>
                ) : (
                  KIND_LABEL[node.kind]
                )}
              </TableCell>
              <TableCell className="folder-table__number">
                {formatBytes(node.size)}
              </TableCell>
              <TableCell className="folder-table__number">
                {formatPercent(node.size, view.size)}
              </TableCell>
              <TableCell className="folder-table__number">
                {node.kind === 'file' ? '' : node.files.toLocaleString()}
              </TableCell>
            </TableRow>
          );
        })}
      </TableBody>
    </Table>
  );
}
