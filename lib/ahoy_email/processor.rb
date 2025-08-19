module AhoyEmail
  module ProcessorExtension

    # extends the original method to add open tracking
    def perform
      track_open if options[:open]

      super
    end

    protected

    # extends the original method to add open tracking
    def track_message
      data = {
        mailer: options[:mailer],
        extra: options[:extra],
        user: options[:user]
      }

      if options[:open]
        data[:token] = token if AhoyEmail.save_token
        data[:campaign] = campaign
      end

      super
    end

    def track_open
      if html_part?
        part = message.html_part || message
        raw_source = part.body.raw_source
        signature = Utils.signature(token: token, campaign: campaign, url: '')

        regex = /<\/body>/i
        url =
          url_for(
            controller: "ahoy/messages",
            action: "open",
            id: token,
            c: campaign,
            s: signature,
            format: "gif"
          )
        pixel = ActionController::Base.helpers.image_tag(url, size: "1x1", alt: "")

        # try to add before body tag
        if raw_source.match(regex)
          part.body = raw_source.gsub(regex, "#{pixel}\\0")
        else
          part.body = raw_source + pixel
        end
      end
    end
  end
end

AhoyEmail::Processor.prepend(AhoyEmail::ProcessorExtension)
