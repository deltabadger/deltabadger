class ParseInterval < BaseService
  Error = StandardError

  INTERVALS = %w[month week day hour minute].freeze

  def call(settings)
    interval = settings.fetch('interval')
    raise Error, 'Invalid interval' if !INTERVALS.include?(interval)

    1.public_send(interval)
  end
end
