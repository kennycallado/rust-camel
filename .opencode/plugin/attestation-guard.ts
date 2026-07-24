import type { Plugin } from "@opencode-ai/plugin"

/**
 * attestation-guard — runtime, fail-fast layer of the attestation-provenance
 * defense. It blocks any subagent that is NOT the conductor-light from writing
 * `.attestation.json` / `.review.json`, whether via the `edit`/`write` tools
 * or via a `bash` command that redirects into one of those files.
 *
 * This is DEFENSE IN DEPTH, not the trust root. The real enforcement is the
 * HMAC signature verified offline by `xtask verify-attestation` in CI — this
 * plugin only makes forgery fail immediately in the live session, and only
 * agents holding $ATTESTATION_HMAC_SECRET can produce a file that survives CI.
 * A worker that disables/edits this plugin still cannot forge a valid HMAC.
 */

const ATTESTATION_FILES = [".attestation.json", ".review.json"]

// Agents permitted to touch attestation files at runtime. The conductor-light
// is the only trusted writer; experts/reviewers return verdicts, they do not
// write the signed file.
const ALLOWED_AGENTS = new Set(["conductor", "conductor-light"])

function mentionsAttestationFile(text: string): boolean {
  return ATTESTATION_FILES.some((f) => text.includes(f))
}

export const AttestationGuard: Plugin = async ({ client }) => {
  // sessionID -> agent name, populated as messages arrive. tool.execute.before
  // does not carry the agent, so we cache it from chat.message and fall back to
  // an SDK session lookup if we somehow miss it.
  const sessionAgent = new Map<string, string>()

  async function agentFor(sessionID: string): Promise<string | undefined> {
    const cached = sessionAgent.get(sessionID)
    if (cached) return cached
    try {
      const res = await client.session.get({ path: { id: sessionID } })
      const agent = (res as any)?.data?.agent ?? (res as any)?.agent
      if (typeof agent === "string") {
        sessionAgent.set(sessionID, agent)
        return agent
      }
    } catch {
      // fail closed below
    }
    return undefined
  }

  return {
    "chat.message": async (input) => {
      if (input.sessionID && input.agent) {
        sessionAgent.set(input.sessionID, input.agent)
      }
    },

    "tool.execute.before": async (input, output) => {
      const { tool, sessionID } = input
      const args = output.args ?? {}

      // Determine the target text depending on the tool surface.
      let target = ""
      if (tool === "edit" || tool === "write") {
        target = String(args.filePath ?? args.path ?? "")
      } else if (tool === "bash") {
        target = String(args.command ?? "")
      } else {
        return // other tools cannot write files
      }

      if (!mentionsAttestationFile(target)) return

      const agent = await agentFor(sessionID)

      // Fail closed: unknown agent identity is treated as untrusted.
      if (agent && ALLOWED_AGENTS.has(agent)) return

      throw new Error(
        `attestation-guard: agent '${agent ?? "unknown"}' may not write ` +
          `attestation files (${ATTESTATION_FILES.join(", ")}). Only the ` +
          `conductor-light signs attestations, via 'xtask sign-attestation' ` +
          `with the HMAC secret. This write was blocked.`,
      )
    },
  }
}
