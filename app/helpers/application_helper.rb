module ApplicationHelper
  def main_body_classes
    classes = []
    classes << 'view--logged-in' if user_signed_in?
    classes << "view--#{controller_name}-#{action_name}"
    classes.join(' ')
  end

  def plan_variant_name(subscription_plan_variant)
    years = subscription_plan_variant.years
    if years.nil?
      subscription_plan_variant.name
    else
      "#{subscription_plan_variant.name} (#{years_amount(years)})"
    end
  end

  def legendary_badger_nft_name(subscription)
    if subscription.nft_rarity.present?
      "#{subscription.nft_name} · #{subscription.nft_rarity}"
    else
      subscription.nft_name
    end
  end

  private

  def action?(controller, action)
    params[:controller] == controller && params[:action] == action
  end

  def years_amount(number)
    "#{number} #{'year'.pluralize(number)}"
  end
end
