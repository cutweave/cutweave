// SPDX-License-Identifier: GPL-3.0-or-later
import { Button } from "./Button.tsx";

export default { title: "Actions/Button", component: Button, tags: ["autodocs"] };

const row = {
  display: "flex",
  gap: "var(--space-3)",
  alignItems: "center",
  padding: "var(--space-4)",
};

export const Variants = {
  render: () => (
    <div style={row}>
      <Button variant="primary">Export video</Button>
      <Button>Add to timeline</Button>
      <Button variant="ghost">Cancel</Button>
      <Button variant="danger">Delete clip</Button>
      <Button variant="agent" icon="sparkles">
        Accept proposal
      </Button>
    </div>
  ),
};

export const Sizes = {
  render: () => (
    <div style={row}>
      <Button size="sm">Small</Button>
      <Button size="md">Medium</Button>
      <Button size="lg" variant="primary">
        Large
      </Button>
    </div>
  ),
};

export const WithIcon = {
  render: () => (
    <div style={row}>
      <Button icon="plus">Add track</Button>
      <Button variant="primary" icon="sparkles">
        Generate clip
      </Button>
    </div>
  ),
};

export const Disabled = {
  render: () => (
    <div style={row}>
      <Button variant="primary" disabled>
        Export video
      </Button>
      <Button disabled>Add to timeline</Button>
      <Button variant="ghost" disabled>
        Cancel
      </Button>
      <Button variant="danger" disabled>
        Delete clip
      </Button>
    </div>
  ),
};
