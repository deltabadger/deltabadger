# §10 IBKR connect wizard (dedicated 2-step Settings flow).
#   show     — render the wizard in whatever state the user's IBKR key is in.
#   create   — step 1: make/reuse a pending IBKR key and generate the OAuth artifacts (bg job).
#   download — serve a PUBLIC artifact (signing/encryption pubkey, or the dhparam) for upload.
#   activate — step 2: paste consumer_key/access_token/secret -> validate -> pending_activation/correct.
#   destroy  — start over (drop the in-progress key).
class Settings::IbkrConnectionsController < ApplicationController
  before_action :authenticate_user!

  # Maps the download param to the private-key column it is derived from (presence-guarded).
  ARTIFACT_COLUMNS = {
    'signing' => :rsa_signature_key,
    'encryption' => :rsa_encryption_key,
    'dhparam' => :dh_param
  }.freeze

  def show
    @api_key = current_ibkr_key
  end

  def create
    @api_key = current_user.api_keys.find_or_create_by!(exchange: ibkr_exchange, key_type: :trading) do |key|
      key.status = :pending_validation
    end
    # Generate only when artifacts are missing — never clobber keys the user may have already
    # uploaded to IBKR by re-submitting step 1.
    Ibkr::GenerateConnectionKeysJob.perform_later(@api_key.id) if @api_key.rsa_signature_key.blank?
    redirect_to settings_ibkr_connect_path
  end

  def download
    api_key = current_ibkr_key
    column = ARTIFACT_COLUMNS[params[:artifact]]
    return redirect_to(settings_ibkr_connect_path) if api_key.nil? || column.nil? || api_key.public_send(column).blank?

    send_data artifact_pem(api_key, params[:artifact]),
              filename: "ibkr_#{params[:artifact]}.pem", type: 'application/x-pem-file', disposition: 'attachment'
  end

  def activate
    @api_key = current_ibkr_key
    return redirect_to(settings_ibkr_connect_path) if @api_key.nil?

    # assign_credentials is destructive (nils absent fields), so re-pass the generated keys to
    # keep them intact while adding the pasted consumer credentials.
    @api_key.validate_credentials!(activate_params.merge(
                                     rsa_signature_key: @api_key.rsa_signature_key,
                                     rsa_encryption_key: @api_key.rsa_encryption_key,
                                     dh_param: @api_key.dh_param,
                                     ibkr_realm: @api_key.ibkr_realm.presence || 'limited_poa'
                                   ))
    set_activation_flash(@api_key)
    redirect_to settings_ibkr_connect_path
  end

  def destroy
    if (api_key = current_ibkr_key)
      api_key.stop_dependent_bots! # don't leave bots firing against a deleted credential
      api_key.destroy
    end
    redirect_to settings_ibkr_connect_path
  end

  private

  def ibkr_exchange
    @ibkr_exchange ||= Exchanges::Ibkr.first
  end

  def current_ibkr_key
    return nil unless ibkr_exchange

    current_user.api_keys.find_by(exchange: ibkr_exchange, key_type: :trading)
  end

  def activate_params
    params.require(:api_key).permit(:key, :access_token, :secret)
  end

  def artifact_pem(api_key, artifact)
    case artifact
    when 'signing'    then OpenSSL::PKey::RSA.new(api_key.rsa_signature_key).public_key.to_pem
    when 'encryption' then OpenSSL::PKey::RSA.new(api_key.rsa_encryption_key).public_key.to_pem
    when 'dhparam'    then api_key.dh_param
    end
  end

  def set_activation_flash(api_key)
    case api_key.status.to_sym
    when :pending_activation
      flash[:notice] = t('settings.ibkr.activation_pending')
    when :correct
      flash[:notice] = t('settings.ibkr.connected')
    when :incorrect
      flash[:alert] = t('settings.ibkr.rejected')
    else
      flash[:alert] = t('settings.ibkr.validation_failed')
    end
  end
end
