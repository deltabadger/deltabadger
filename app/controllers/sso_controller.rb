class SsoController < ApplicationController
  def sso
    secret = ENV['DISCOURSE_SSO_SECRET']
    discourse_sso_url = ENV['DISCOURSE_SSO_URL']

    if secret.nil? || discourse_sso_url.nil?
      Rails.logger.error 'Discourse SSO secret or URL not set'
      return render plain: 'Configuration error', status: :internal_server_error
    end

    if request.query_string.blank?
      Rails.logger.error 'Query string is empty'
      return render plain: 'Query string is empty', status: :bad_request
    end

    begin
      sso = DiscourseApi::SingleSignOn.parse(request.query_string, secret)
    rescue DiscourseApi::SingleSignOn::ParseError => e
      Rails.logger.error "Failed to parse SSO: #{e.message}"
      return render plain: "SSO parse error: #{e.message}", status: :bad_request
    end

    user = current_user

    if user.nil?
      Rails.logger.error 'No user logged in'
      return render plain: 'No user logged in', status: :unauthorized
    end

    # Check the subscription plan ID
    active_subscription = user.active_subscription
    unless active_subscription && active_subscription.subscription_plan_id != 1
      Rails.logger.error 'User does not have an eligible subscription plan for SSO'
      return render plain: 'Upgrade your plan to access the community', status: :forbidden
    end

    # Assign Discourse badges based on the subscription plan
    assign_badges_to_user(user, active_subscription)

    # Create a temporary username from the first word of user's name + user.id
    first_name = user.name.split.first.downcase
    temp_username = "#{first_name}#{user.id}"

    if user.email.nil? || user.name.nil? || user.id.nil? || temp_username.nil?
      Rails.logger.error 'User attribute is missing'
      return render plain: 'User attribute is missing', status: :internal_server_error
    end

    sso.email = user.email
    sso.name = user.name
    sso.username = temp_username # Use the temporary username
    sso.external_id = user.id.to_s
    sso.sso_secret = secret

    redirect_url = sso.to_url(discourse_sso_url)

    redirect_to redirect_url
  end

  private

  def assign_badges_to_user(user, active_subscription)
    discourse_api_key = ENV['DISCOURSE_API_KEY']
    discourse_api_username = ENV['DISCOURSE_API_USERNAME']
    discourse_site_url = ENV['DISCOURSE_SITE_URL']
    discourse = DiscourseApi::Client.new(discourse_site_url)
    discourse.api_key = discourse_api_key
    discourse.api_username = discourse_api_username

    legendary_plan_badge_id = 103 # Replace with the actual badge ID for "Legendary Badger"

    # First, remove any badge the user shouldn't have
    return unless active_subscription.subscription_plan_id == 4

    discourse.user_badges.grant(user.id, legendary_plan_badge_id)
  end
end
