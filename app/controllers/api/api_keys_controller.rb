module Api
  class ApiKeysController < Api::BaseController
    def create
      keys_params = api_key_params.merge(user: current_user)
      # A pasted Coinbase CDP PEM secret arrives with literal \n sequences instead of real
      # newlines; unescape them so OpenSSL::PKey::EC can parse the key.
      keys_params[:key] = keys_params[:key].to_s.gsub('\n', "\n")
      keys_params[:secret] = keys_params[:secret].to_s.gsub('\n', "\n")
      api_key = current_user.api_keys
                            .find_by(exchange_id: keys_params[:exchange_id], key_type: keys_params[:key_type])

      result = if api_key.nil?
                 AddApiKey.call(keys_params)
               elsif same_keys?(keys_params, api_key)
                 revalidate_api_key(api_key)
               else
                 replace(api_key, keys_params)
               end

      if result.success?
        render json: { data: true }, status: 201
      else
        render json: { data: false }, status: 422
      end
    end

    def remove_invalid_keys
      api_key = current_user.api_keys.find_by(exchange_id: invalid_key_params[:exchange_id])
      return if api_key.nil? || !api_key.incorrect?

      remove(api_key)
    end

    private

    def revalidate_api_key(api_key)
      api_key.update(status: 'pending_validation')
      ApiKeyValidatorJob.perform_later(api_key.id)
      Result::Success.new
    rescue StandardError
      Result::Failure.new
    end

    # In place, so the key keeps its id and the ledger rows linked to it. The update validates
    # before it writes: a rejected replacement leaves the stored key exactly as it was. The ledger
    # watermark belonged to the old credential, which may have been another account: the new one
    # syncs its own full history.
    def replace(api_key, params)
      replaced = api_key.update(key: params[:key], secret: params[:secret], passphrase: params[:passphrase],
                                german_trading_agreement: params[:german_trading_agreement],
                                status: :pending_validation, last_sync_error: nil, last_synced_at: nil)
      return Result::Failure.new(api_key.errors.full_messages) unless replaced

      ApiKeyValidatorJob.perform_later(api_key.id)
      Result::Success.new(api_key)
    end

    def remove(api_key)
      api_key.destroy!
    end

    def api_key_params
      params.require(:api_key).permit(:key, :secret, :passphrase, :exchange_id, :german_trading_agreement, :key_type)
    end

    def invalid_key_params
      params.permit(:exchange_id)
    end

    def same_keys?(params, api_key)
      params[:key] == api_key.key && params[:secret] == api_key.secret
    end
  end
end
