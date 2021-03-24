module WireTransfer::Validators
  class Create
    def call(params)
      return Result::Failure.new if invalid_wire_params(params)

      Result::Success.new(params)
    end

    private

    def invalid_wire_params(params)
      params[:first_name].blank? || params[:last_name].blank? || params[:address].blank?
    end
  end
end
