module Article::Paywall
  extend ActiveSupport::Concern

  def has_paywall?
    content.include?(paywall_marker)
  end

  def free_content
    return content unless has_paywall?

    split_content_at_paywall(content)[:free]
  end

  def premium_content
    return '' unless has_paywall?

    split_content_at_paywall(content)[:premium]
  end

  def user_has_access?(user)
    return false unless user&.persisted?

    # Paid users (basic, pro, legendary) can see all premium content
    user.subscription.paid?
  end

  def paywall_plan_required
    # You can customize this based on article requirements
    # For now, any paid plan gives access
    'basic'
  end

  private

  def split_content_at_paywall(content)
    parts = content.split(paywall_marker, 2)

    {
      free: parts[0]&.strip || '',
      premium: parts[1]&.strip || ''
    }
  end
end
