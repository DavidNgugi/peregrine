import type { ReactElement } from "react";
import { useTranslation } from "react-i18next";
import { SetupStep } from "./SetupStep.js";

/** The one screen that asks for nothing, and says what the application is responsible for. */
export function WelcomeStep(): ReactElement {
  const { t } = useTranslation();
  return <SetupStep body={t("setup.welcome.body")} title={t("setup.welcome.title")} />;
}
