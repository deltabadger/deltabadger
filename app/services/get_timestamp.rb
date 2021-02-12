class GetTimestamp < BaseService
  def call(millis: true)
    format = '%s'
    format += '%L' if millis
    Time.now.strftime(format)
  end
end
