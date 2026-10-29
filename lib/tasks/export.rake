require_relative '../export_scratch'

namespace :export do
  task :scratch do
    ExportScratch.validate!
  end

  desc 'Export a stopped installation copy from an isolated scratch container'
  task portable: %i[scratch environment] do
    Export::Portable.new.call
    puts 'Portable export complete: /export/manifest.json'
  end
end
