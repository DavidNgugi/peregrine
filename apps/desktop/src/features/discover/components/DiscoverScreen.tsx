import { EmptyState, Screen } from "@peregrine/ui";
import type { ReactElement } from "react";
import { useTranslation } from "react-i18next";

/**
 * The Discover screen, which is an empty state until discovery has data.
 *
 * It says what the screen is and that it has no catalogue yet, and computes nothing. The
 * hero and the rails are PG-E7-08's and arrive with the provider cache behind them (PG-E5,
 * PG-E6); the designed treatment of each of their states is PG-E7-16's. What this screen
 * owns today is the one state it can be in — and it is a *designed* state rather than a
 * spinner, per §11 rule 2.
 *
 * The empty state is centred because the screen has nothing else on it, which is the same
 * reason it takes the page size: a screen's own empty state carries the screen.
 */
export function DiscoverScreen(): ReactElement {
  const { t } = useTranslation();

  return (
    <Screen title={t("discover.title")}>
      <EmptyState
        body={t("discover.empty.body")}
        className="m-auto max-w-md items-center text-center"
        size="page"
        title={t("discover.empty.title")}
      />
    </Screen>
  );
}
