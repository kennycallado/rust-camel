FROM denoland/deno:2.1.4
WORKDIR /app
COPY main.ts camel.ts ./
EXPOSE 8080
ENV PORT=8080
ENV DENO_NO_PROMPT=1
ENV TINI_SUBREAPER=true
CMD ["deno", "run", "--allow-net=0.0.0.0", "--allow-env=PORT", "main.ts"]
