import type { ReactElement } from "react";
import type { CastMemberDto } from "@peregrine/ipc-client";
import { useTranslation } from "react-i18next";
import { Section } from "@peregrine/ui";
import { imageUrl } from "../../../lib/media.js";

/**
 * The top-billed cast, as portraits.
 *
 * A person without a cached portrait gets initials rather than a broken image: the credit is the
 * information and the photograph is an improvement on it, so a cache miss must not hide the name.
 */
export function CastRail({
  cast,
  onPerson,
}: {
  cast: readonly CastMemberDto[];
  /** Opens the person's page; a rail drawn without it is still readable, just not clickable. */
  onPerson?: (id: number) => void;
}): ReactElement | null {
  const { t } = useTranslation();
  if (cast.length === 0) {
    return null;
  }
  return (
    <Section description={t("title.cast.description")} title={t("title.cast.title")}>
      <ul className="no-scrollbar flex gap-5 overflow-x-auto pb-2" data-title-cast="">
        {cast.map((member) => (
          <li className="w-24 shrink-0" key={`${member.name}-${member.character ?? ""}`}>
            <button
              className="flex w-full flex-col items-center gap-1.5 text-center"
              disabled={onPerson === undefined}
              onClick={() => {
                onPerson?.(member.id);
              }}
              type="button"
            >
              {member.profile === null ? (
                <span
                  aria-hidden="true"
                  className="flex size-16 items-center justify-center rounded-full bg-surface-2 text-body font-medium text-text-secondary"
                >
                  {initials(member.name)}
                </span>
              ) : (
                <img
                  alt=""
                  className="size-16 rounded-full object-cover"
                  loading="lazy"
                  src={imageUrl(member.profile) ?? undefined}
                />
              )}
              <span className="text-caption font-medium text-text-primary">{member.name}</span>
              {member.character === null ? null : (
                <span className="text-caption text-text-tertiary">{member.character}</span>
              )}
            </button>
          </li>
        ))}
      </ul>
    </Section>
  );
}

function initials(name: string): string {
  const parts = name.trim().split(/\s+/).slice(0, 2);
  return parts.map((part) => part.charAt(0).toUpperCase()).join("");
}
