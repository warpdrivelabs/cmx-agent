/** 两旋钮（对齐 protocol.rs SetPolicy：SandboxMode × ApprovalPolicy 正交，其余 Policy 项不动）。 */

export interface Policy {
  sandbox: SandboxMode;
  approval: ApprovalPolicy;
}

export type SandboxMode = "read-only" | "workspace-write" | "danger-full-access";
export type ApprovalPolicy = "never" | "on-request" | "unless-trusted";

export const SANDBOX_MODES: readonly SandboxMode[] = [
  "read-only",
  "workspace-write",
  "danger-full-access"
];

export const APPROVAL_POLICIES: readonly ApprovalPolicy[] = [
  "never",
  "on-request",
  "unless-trusted"
];
