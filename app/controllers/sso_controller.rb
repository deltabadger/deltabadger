class SsoController < ApplicationController
  def sso
    secret = ENV['DISCOURSE_SSO_SECRET']
    discourse_sso_url = ENV['DISCOURSE_SSO_URL']

    if secret.nil? || discourse_sso_url.nil?
      Rails.logger.error 'Discourse SSO secret or URL not set'
      return render plain: 'Configuration error: Discourse SSO secret or URL not set', status: :internal_server_error
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

    # Check if the user has a valid subscription with plan_id 3 or 4
    valid_subscription = user.subscriptions.current.where(subscription_plan_id: [3, 4]).exists?

    unless valid_subscription
      Rails.logger.error 'User does not have a valid subscription'
      return render plain: 'Access denied: You need an appropriate subscription to access this feature', status: :forbidden
    end

    temp_username = "badger#{user.id}"

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
end
