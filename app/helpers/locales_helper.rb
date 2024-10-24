module LocalesHelper
  def localized_plan_name(name)
    t("subscriptions.#{name}")
  end
end
