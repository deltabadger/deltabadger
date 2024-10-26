module ApplicationHelper
  def main_html_classes
    classes = []
    classes << 'view--logged-in' if user_signed_in?
    classes << "view--#{controller_name}-#{action_name}"
    classes.join(' ')
  end

  def render_turbo_stream_flash_messages
    turbo_stream.prepend 'flash', partial: 'layouts/flash'
  end

  def show_limit_reached_navbar?
    current_user.limit_reached? &&
      !action?('upgrade', 'index') &&
      !action?('affiliates', 'new') &&
      !action?('affiliates', 'create')
  end

  def plan_variant_name(subscription_plan_variant)
    years = subscription_plan_variant.years
    if years.nil?
      subscription_plan_variant.name
    else
      "#{subscription_plan_variant.name} (#{years} #{'year'.pluralize(years)})"
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
end
