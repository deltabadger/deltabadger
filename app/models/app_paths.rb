# frozen_string_literal: true

class AppPaths
  class << self
    def tmp
      ENV['APP_TMP_DIR'].presence || Rails.root.join('tmp')
    end
  end
end
