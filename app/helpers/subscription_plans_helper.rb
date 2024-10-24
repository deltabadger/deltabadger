module SubscriptionPlansHelper
  def localized_plan_name(name)
    I18n.t("subscriptions.#{name}")
  end
end
