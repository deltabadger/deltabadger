module Api
  class FrequencyController < ApplicationController
    def limit_exceeded
      limit_exceeded = CheckExceededFrequency.call(params[:exchange_id], params[:type], params[:price], params[:base], params[:quote], params[:currency_of_minimum], params[:interval], params[:smart_intervals_value])
      render json: {limit_exceeded: limit_exceeded}.to_json
    end
  end
end
