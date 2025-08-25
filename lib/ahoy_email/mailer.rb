module AhoyEmail
  module Mailer
    class_methods do
      def track_open(**options)
        raise ArgumentError, "missing keyword: :campaign" unless options.key?(:campaign)
        set_ahoy_options(options, :open)
      end
    end

    # overwrites the original method to add open tracking
    def save_ahoy_options
      Safely.safely do
        options = {}
        call_ahoy_options(options, :message)
        call_ahoy_options(options, :utm_params)
        call_ahoy_options(options, :click)
        call_ahoy_options(options, :open)

        if options[:message] || options[:utm_params] || options[:click] || options[:open]
          AhoyEmail::Processor.new(self, options).perform
        end
      end
    end
  end
end
