module WireTransfer
  class Create < BaseService
    def initialize(
      wire_transfer_validator: WireTransfer::Validators::Create.new
    )
      @wire_transfer_validator = wire_transfer_validator
    end

    def call(params, additional_params)
      params = wire_transfer_params(params, additional_params)
      validated_params = @wire_transfer_validator.call(params)
      return validated_params unless validated_params.success?

      WireTransferMailer.with(
        wire_params: validated_params.data
      ).new_wire_transfer.deliver_later

      Result::Success.new
    end

    private

    def subscription_plan_repository
      @subscription_plan_repository ||= SubscriptionPlansRepository.new
    end

    def wire_transfer_price_params(params, country, plan)
      cost_presenter = if plan == 'hodler'
                         params[:cost_presenters][country][:hodler]
                       else
                         params[:cost_presenters][country][:investor]
                       end
      {
        referral_code: params[:referrer].nil? ? nil : params[:referrer].code,
        price: cost_presenter.total_price,
        discount: (cost_presenter.base_price_with_vat.to_f - cost_presenter.total_price.to_f).round(2)
      }
    end

    def wire_transfer_params(params, additional_params)
      country = params[:country]
      plan = subscription_plan_repository.find(params[:subscription_plan_id]).name

      {
        first_name: params[:first_name],
        last_name: params[:last_name],
        address: params[:address],
        user_email: params[:user].email,
        country: country,
        vat_number: params.fetch(:vat_number, nil),
        subscription_plan: plan
      }.merge(wire_transfer_price_params(additional_params, country, plan))
    end
  end
end
