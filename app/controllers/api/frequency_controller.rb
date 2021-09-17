module Api
  class FrequencyController < ApplicationController
    def limit_exceeded
      limit_exceeded = CheckExceededFrequency.call(params[:exchange_id], params[:type], params[:price], params[:base], params[:quote], params[:currency_of_minimum], params[:interval], params[:forceSmartIntervals], params[:smartIntervalsValue])
      render json: { limit_exceeded: limit_exceeded[:limit_exceeded], new_intervals_value: limit_exceeded[:new_intervals_value], frequency_limit: limit_exceeded[:frequency_limit]}.to_json
    end
  end
end
