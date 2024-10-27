module LocalesHelper
  def localized_plan_name(name)
    t("subscriptions.#{name}")
  end

  def localized_payment_country_options
    VatRate.all_in_display_order.map do |vat_rate|
      [
        vat_rate.country == VatRate::NOT_EU ? t('helpers.label.payment.other') : vat_rate.country,
        vat_rate.country
      ]
    end
  end
end
