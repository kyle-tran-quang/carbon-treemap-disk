import {
  Accordion,
  AccordionItem,
  StructuredListBody,
  StructuredListCell,
  StructuredListHead,
  StructuredListRow,
  StructuredListWrapper,
} from '@carbon/react';
import type { ScanIssue } from '../ipc';

interface ScanIssueListProps {
  issues: ScanIssue[];
}

/** Paths the scan could not read. They are listed, never treated as failures. */
export function ScanIssueList({ issues }: ScanIssueListProps) {
  if (issues.length === 0) return null;

  const title = `${issues.length.toLocaleString()} ${
    issues.length === 1 ? 'item' : 'items'
  } not accessible`;

  return (
    <Accordion className="app__issues">
      <AccordionItem title={title}>
        <StructuredListWrapper isCondensed aria-label="Items not accessible">
          <StructuredListHead>
            <StructuredListRow head>
              <StructuredListCell head>Path</StructuredListCell>
              <StructuredListCell head>Reason</StructuredListCell>
            </StructuredListRow>
          </StructuredListHead>
          <StructuredListBody>
            {issues.map((issue) => (
              <StructuredListRow key={issue.path}>
                <StructuredListCell className="app__issue-path">
                  {issue.path}
                </StructuredListCell>
                <StructuredListCell>{issue.message}</StructuredListCell>
              </StructuredListRow>
            ))}
          </StructuredListBody>
        </StructuredListWrapper>
      </AccordionItem>
    </Accordion>
  );
}
