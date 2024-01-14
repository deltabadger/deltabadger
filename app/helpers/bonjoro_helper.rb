module BonjoroHelper
  require 'openssl'
  require 'base64'

  BONJORO_SECRET_KEY = ENV.fetch('BONJORO_SECRET_KEY').freeze
  BONJORO_APP_ID = ENV.fetch('BONJORO_APP_ID').freeze

  def bonjoro_user_hash(email)
    OpenSSL::HMAC.hexdigest('sha256', BONJORO_SECRET_KEY, email)
  end

  def bonjoro_app_id
    BONJORO_APP_ID
  end
end
