class SsoController < ApplicationController
  def sso
    secret = ENV['DISCOURSE_SSO_SECRET']
    sso = DiscourseApi::SingleSignOn.parse(request.query_string, secret)

    user = current_user

    sso.email = user.email
    sso.name = user.name
    sso.username = user.username
    sso.external_id = user.id.to_s
    sso.sso_secret = secret

    discourse_sso_url = ENV['DISCOURSE_SSO_URL']
    redirect_to sso.to_url(discourse_sso_url)
  end
end
