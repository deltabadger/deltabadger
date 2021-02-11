class GetTimestamp < BaseService
  def call(millis: true)
    return Time.now.utc.to_i.to_s unless millis

    Time.now.strftime('%s%L')
  end
end
